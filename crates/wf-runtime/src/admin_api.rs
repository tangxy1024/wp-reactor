//! Admin API server — minimal HTTP API for engine status.
//!
//! Protected by bearer token authentication.

use std::convert::Infallible;
use std::fs;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use http_body_util::Full;
use hyper::body::Bytes;
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as AutoBuilder;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use uuid::Uuid;
use wf_config::AdminApiConf;

use crate::error::{RuntimeReason, RuntimeResult};

// ── AdminApiRuntime ───────────────────────────────────────────────────

pub struct AdminApiRuntime {
    local_addr: SocketAddr,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl AdminApiRuntime {
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.task.await;
    }
}

// ── AppState ──────────────────────────────────────────────────────────

struct AppState {
    bearer_token: String,
    instance_id: String,
    version: String,
}

// ── Start ─────────────────────────────────────────────────────────────

fn conf_err(detail: impl Into<String>) -> crate::error::RuntimeError {
    RuntimeReason::core_conf()
        .fail::<()>(detail.into())
        .unwrap_err()
}

pub async fn start_if_enabled(
    work_root: &Path,
    config: &AdminApiConf,
) -> RuntimeResult<Option<AdminApiRuntime>> {
    if !config.enabled {
        return Ok(None);
    }

    let bind: SocketAddr = config
        .bind
        .parse()
        .map_err(|e| conf_err(format!("invalid admin_api.bind \"{}\": {e}", config.bind)))?;

    let listener = TcpListener::bind(bind)
        .await
        .map_err(|e| conf_err(format!("bind admin api on {bind}: {e}")))?;

    let local_addr = listener
        .local_addr()
        .map_err(|e| conf_err(format!("read admin api local addr: {e}")))?;

    let token_path = work_root.join(&config.auth.token_file);
    let bearer_token = fs::read_to_string(&token_path)
        .map_err(|e| conf_err(format!("read token file {}: {e}", token_path.display())))?
        .trim()
        .to_string();

    if bearer_token.is_empty() {
        return Err(conf_err("admin_api token file is empty"));
    }

    let instance_id = format!("fusion:{}", std::process::id());

    let state = Arc::new(AppState {
        bearer_token,
        instance_id,
        version: env!("CARGO_PKG_VERSION").to_string(),
    });

    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let task = tokio::spawn(run_plain(listener, state, shutdown_rx));

    Ok(Some(AdminApiRuntime {
        local_addr,
        shutdown_tx: Some(shutdown_tx),
        task,
    }))
}

// ── Server ────────────────────────────────────────────────────────────

async fn run_plain(
    listener: TcpListener,
    state: Arc<AppState>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, remote_addr)) => {
                        let state = state.clone();
                        tokio::spawn(async move {
                            let io = TokioIo::new(stream);
                            if let Err(err) = AutoBuilder::new(TokioExecutor::new())
                                .serve_connection(io, hyper::service::service_fn(move |req| {
                                    handle_request(req, remote_addr, state.clone())
                                }))
                                .await
                            {
                                tracing::warn!(domain = "sys", "admin api connection error: {err}");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::warn!(domain = "sys", "admin api accept error: {e}");
                    }
                }
            }
            _ = &mut shutdown_rx => {
                tracing::info!(domain = "sys", "admin api shutting down");
                break;
            }
        }
    }
}

// ── Request handling ──────────────────────────────────────────────────

async fn handle_request(
    req: Request<hyper::body::Incoming>,
    remote_addr: SocketAddr,
    state: Arc<AppState>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let request_id = Uuid::new_v4().to_string();
    let path = req.uri().path().to_string();
    let method = req.method().clone();

    // Bearer token auth
    let auth_header = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let expected = format!("Bearer {}", state.bearer_token);
    if auth_header != expected {
        let body = format!(
            r#"{{"request_id":"{}","accepted":false,"result":"unauthorized","error":"invalid bearer token"}}"#,
            request_id
        );
        return Ok(json_response(StatusCode::UNAUTHORIZED, &body));
    }

    match (method.clone(), path.as_str()) {
        (Method::GET, "/admin/v1/runtime/status") => {
            tracing::info!(
                domain = "sys",
                "admin api status request_id={} remote={}",
                request_id,
                remote_addr
            );
            let body = format!(
                r#"{{"instance_id":"{}","version":"{}","accepting":true}}"#,
                state.instance_id, state.version
            );
            Ok(json_response(StatusCode::OK, &body))
        }
        _ => {
            let body = format!(
                r#"{{"request_id":"{}","accepted":false,"result":"not_found","error":"unsupported route {} {}"}}"#,
                request_id, method, path
            );
            Ok(json_response(StatusCode::NOT_FOUND, &body))
        }
    }
}

fn json_response(status: StatusCode, body: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Full::from(body.to_string()))
        .unwrap()
}
