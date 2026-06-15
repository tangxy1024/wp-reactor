use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Once};
use std::time::Duration;

use orion_error::conversion::{SourceErr, ToStructError};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use wf_config::FusionConfig;
use wf_engine::alert::OutputRecord;
use wf_engine::sink::SinkDispatcher;
use wf_engine::window::{Evictor, Router, WindowRegistry};

use crate::alert_task;
use crate::engine_task::{RuleTaskConfig, WindowSource, run_rule_task};
use crate::error::{RuntimeReason, RuntimeResult};
use crate::evictor_task;
use crate::metrics::{MetricsRecord, MonRecv, RuntimeMetrics, run_metrics_task};
use crate::receiver::{
    Receiver, replay_arrow_framed_file, replay_arrow_ipc_file, replay_csv_file, replay_ndjson_file,
    resolve_stream_schema,
};
use wp_model_core::model::{DataRecord, DataType, Field, FieldStorage, Value};

use super::types::{RunRule, RunRuleKind, TaskGroup};

// ---------------------------------------------------------------------------
// Phase 2: task spawn helpers — each creates channel + spawns task
// ---------------------------------------------------------------------------

/// Spawn the alert pipeline: build channel, spawn consumer task.
/// Returns (alert_tx, task_group).
pub(super) fn spawn_alert_task(
    dispatcher: Arc<SinkDispatcher>,
    metrics: Option<Arc<RuntimeMetrics>>,
) -> (mpsc::Sender<OutputRecord>, TaskGroup) {
    let (alert_tx, alert_rx) = mpsc::channel(alert_task::ALERT_CHANNEL_CAPACITY);
    let mut group = TaskGroup::new("alert");
    group.push(tokio::spawn(async move {
        alert_task::run_alert_dispatcher(alert_rx, dispatcher, metrics).await;
        Ok(())
    }));
    (alert_tx, group)
}

/// Spawn the periodic window evictor task.
pub(super) fn spawn_evictor_task(
    config: &FusionConfig,
    router: &Arc<Router>,
    cancel: CancellationToken,
    metrics: Option<Arc<RuntimeMetrics>>,
) -> TaskGroup {
    let evictor = Evictor::new(config.window_defaults.max_total_bytes.as_bytes());
    let evict_interval = config.window_defaults.evict_interval.as_duration();
    let router = Arc::clone(router);
    let mut group = TaskGroup::new("evictor");
    group.push(tokio::spawn(async move {
        evictor_task::run_evictor(evictor, router, evict_interval, cancel, metrics).await;
        Ok(())
    }));
    group
}

/// Spawn one independent task per compiled rule.
///
/// Each rule task owns its `CepStateMachine` exclusively (no `Arc<Mutex>`).
/// It subscribes to window notifications and uses cursor-based `read_since()`
/// to pull new batches.
pub(super) fn spawn_rule_tasks(
    rules: Vec<RunRule>,
    router: &Arc<Router>,
    intermediate_targets: &HashSet<String>,
    alert_tx: mpsc::Sender<OutputRecord>,
    cancel: CancellationToken,
    metrics: Option<Arc<RuntimeMetrics>>,
) -> TaskGroup {
    let mut group = TaskGroup::new("rules");
    let timeout_scan_interval = Duration::from_secs(1);

    for rule in rules {
        let (machine, each_alias, each_time_field) = match rule.kind {
            RunRuleKind::Match(machine) => (Some(*machine), None, None),
            RunRuleKind::Each { alias, time_field } => (None, Some(alias), time_field),
        };
        let window_sources = resolve_window_sources(&rule.window_aliases, router.registry());

        let task_config = RuleTaskConfig {
            machine,
            each_alias,
            each_time_field,
            executor: rule.executor,
            window_sources,
            alert_tx: alert_tx.clone(),
            cancel: cancel.child_token(),
            timeout_scan_interval,
            router: Arc::clone(router),
            metrics: metrics.clone(),
            intermediate_targets: intermediate_targets.clone(),
        };

        group.push(tokio::spawn(
            async move { run_rule_task(task_config).await },
        ));
    }

    // Drop our copy of alert_tx so the alert channel closes when all rule
    // tasks finish.
    drop(alert_tx);

    group
}

/// Resolve which windows a rule needs to subscribe to, based on its direct
/// bind.window → alias mapping.
pub(super) fn resolve_window_sources(
    window_aliases: &HashMap<String, Vec<String>>,
    registry: &WindowRegistry,
) -> Vec<WindowSource> {
    let mut sources = Vec::new();

    for (window_name, aliases) in window_aliases {
        if let Some(window) = registry.get_window(window_name)
            && let Some(notify) = registry.get_notifier(window_name)
        {
            sources.push(WindowSource {
                window_name: window_name.clone(),
                window: Arc::clone(window),
                notify: Arc::clone(notify),
                aliases: aliases.clone(),
            });
        }
    }

    sources
}

/// Bind the receiver and spawn its task.
/// Returns (listen_addr, task_group).
pub(super) async fn spawn_receiver_task(
    config: &FusionConfig,
    router: Arc<Router>,
    cancel: CancellationToken,
    metrics: Option<Arc<RuntimeMetrics>>,
    schemas: &[wf_lang::WindowSchema],
    base_dir: &Path,
) -> RuntimeResult<(Option<SocketAddr>, TaskGroup)> {
    let mut group = TaskGroup::new("receiver");
    let mut listen_addr: Option<SocketAddr> = None;
    let mut spawned = 0usize;
    let schema_catalog = Arc::new(schemas.to_vec());
    register_builtin_external_sources();

    for source in &config.sources {
        if !source.enabled {
            continue;
        }
        match source.kind() {
            "tcp" => {
                let listen = source
                    .params
                    .get("listen")
                    .map(|s| s.as_str())
                    .unwrap_or("");
                let stream_name = source.params.get("stream").cloned().unwrap_or_default();
                let format = source
                    .params
                    .get("format")
                    .cloned()
                    .unwrap_or_else(|| "arrow_stream".into());

                match format.as_str() {
                    "arrow_framed" => {
                        // Legacy: length-prefix frames with wp_arrow::ipc::decode_ipc
                        let receiver = Receiver::bind(listen, Arc::clone(&router), metrics.clone())
                            .await
                            .source_err(RuntimeReason::system_error(), "bind tcp receiver")?;
                        let bound = receiver.local_addr().source_err(
                            RuntimeReason::system_error(),
                            "read tcp receiver local address",
                        )?;
                        if listen_addr.is_none() {
                            listen_addr = Some(bound);
                        }
                        let receiver_cancel = receiver.cancel_token();
                        let cancel_child = cancel.child_token();
                        tokio::spawn(async move {
                            cancel_child.cancelled().await;
                            receiver_cancel.cancel();
                        });
                        group.push(tokio::spawn(async move { receiver.run().await }));
                        spawned += 1;
                    }
                    _ => {
                        // Default: Arrow IPC Stream (StreamReader, no length prefix)
                        let addr = listen.strip_prefix("tcp://").unwrap_or(listen);
                        let read_timeout_secs: u64 = source
                            .params
                            .get("read_timeout_secs")
                            .and_then(|v| v.parse::<u64>().ok())
                            .unwrap_or(30);
                        let listener = tokio::net::TcpListener::bind(addr).await.source_err(
                            RuntimeReason::system_error(),
                            format!("bind tcp listener {addr}"),
                        )?;
                        let bound = listener.local_addr().source_err(
                            RuntimeReason::system_error(),
                            "read tcp listener local address",
                        )?;
                        if listen_addr.is_none() {
                            listen_addr = Some(bound);
                        }
                        let router = Arc::clone(&router);
                        let metrics = metrics.clone();
                        let cancel = cancel.child_token();
                        group.push(tokio::spawn(async move {
                            loop {
                                tokio::select! {
                                    result = listener.accept() => {
                                        match result {
                                            Ok((stream, peer)) => {
                                                use crate::receiver::handle_connection_stream;
                                                handle_connection_stream(
                                                    stream,
                                                    stream_name.clone(),
                                                    router.clone(),
                                                    metrics.clone(),
                                                    cancel.child_token(),
                                                    peer,
                                                    read_timeout_secs,
                                                ).await;
                                            }
                                            Err(e) => {
                                                wf_warn!(conn, error = %e, "accept tcp connection error");
                                            }
                                        }
                                    }
                                    _ = cancel.cancelled() => break,
                                }
                            }
                            Ok(())
                        }));
                        spawned += 1;
                    }
                }
            }
            "file" => {
                let path_str = source.params.get("path").map(|s| s.as_str()).unwrap_or("");
                let path = resolve_source_path(base_dir, path_str);
                let stream = source.params.get("stream").cloned().unwrap_or_default();
                let router = Arc::clone(&router);
                let metrics = metrics.clone();
                let cancel = cancel.child_token();
                let format = source
                    .params
                    .get("format")
                    .cloned()
                    .unwrap_or_else(|| "ndjson".into());
                let schemas = Arc::clone(&schema_catalog);
                group.push(tokio::spawn(async move {
                    match format.as_str() {
                        "ndjson" => {
                            replay_ndjson_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?
                        }
                        "csv" => {
                            replay_csv_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?
                        }
                        "arrow_framed" => {
                            replay_arrow_framed_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?
                        }
                        "arrow_ipc" => {
                            replay_arrow_ipc_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?
                        }
                        _ => {
                            return Err(RuntimeReason::system_error()
                                .to_err()
                                .with_detail(format!("unsupported format: {format}")));
                        }
                    }
                    Ok(())
                }));
                spawned += 1;
            }
            other => {
                spawned += spawn_external_source_tasks(
                    source,
                    other,
                    spawned,
                    base_dir,
                    &schema_catalog,
                    &router,
                    metrics.clone(),
                    cancel.child_token(),
                    &mut group,
                )
                .await?;
            }
        }
    }

    if spawned == 0 {
        return RuntimeReason::Bootstrap
            .to_err()
            .with_detail("no enabled sources configured")
            .err();
    }

    Ok((listen_addr, group))
}

fn resolve_source_path(base_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

fn register_builtin_external_sources() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        wp_core_connectors::sources::register_file_factory();
        wp_core_connectors::sources::tcp::register_tcp_factory();
        wp_core_connectors::sources::syslog::register_syslog_factory();
    });
}

#[allow(clippy::too_many_arguments)]
async fn spawn_external_source_tasks(
    source: &wf_config::SourceConfig,
    source_kind: &str,
    source_idx: usize,
    base_dir: &Path,
    schemas: &Arc<Vec<wf_lang::WindowSchema>>,
    router: &Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
    group: &mut TaskGroup,
) -> RuntimeResult<usize> {
    let Some(factory) = wp_core_connectors::registry::get_source_factory(source_kind) else {
        return RuntimeReason::Bootstrap
            .to_err()
            .with_detail(format!(
                "no factory registered for source kind {source_kind:?}"
            ))
            .err();
    };

    let stream_name = source.params.get("stream").cloned().unwrap_or_default();
    let schema = resolve_stream_schema(schemas.as_slice(), &stream_name)?;
    let mut params = wp_connector_api::ParamMap::new();
    for (key, value) in &source.params {
        params.insert(key.clone(), source_param_to_json(value));
    }
    let source_spec = wp_connector_api::SourceSpec {
        name: source.effective_name(source_idx),
        kind: source_kind.to_string(),
        connector_id: String::new(),
        params,
        tags: Vec::new(),
    };

    factory.validate_spec(&source_spec).source_err(
        RuntimeReason::Bootstrap,
        format!("validate source {:?}", source_spec.name),
    )?;

    let mut svc = factory
        .build(
            &source_spec,
            &wp_connector_api::SourceBuildCtx::new(base_dir.to_path_buf()),
        )
        .await
        .source_err(
            RuntimeReason::Bootstrap,
            format!("build source {:?}", source_spec.name),
        )?;

    let mut spawned = 0usize;
    if let Some(mut acceptor) = svc.acceptor.take() {
        let cancel = cancel.child_token();
        group.push(tokio::spawn(async move {
            let (ctrl_tx, ctrl_rx) = async_broadcast::broadcast(1);
            tokio::select! {
                result = acceptor.acceptor.accept_connection(ctrl_rx) => {
                    result.map_err(|e| RuntimeReason::system_error().to_err().with_source(e))
                }
                _ = cancel.cancelled() => {
                    let _ = ctrl_tx.broadcast(wp_connector_api::ControlEvent::Stop).await;
                    Ok(())
                },
            }
        }));
        spawned += 1;
    }

    for mut handle in svc.sources {
        let router = Arc::clone(router);
        let metrics = metrics.clone();
        let cancel = cancel.child_token();
        let stream_name = stream_name.clone();
        let source_kind = source_kind.to_string();
        let schema = Arc::clone(&schema);
        group.push(tokio::spawn(async move {
            let mut consecutive_errors: u32 = 0;
            loop {
                tokio::select! {
                    result = handle.source.receive() => {
                        match result {
                            Ok(batch) if !batch.is_empty() => {
                                consecutive_errors = 0;
                                for event in batch {
                                    let payload = wp_core_connectors::sources::batch::payload::payload_to_string(&event.payload);
                                    let trimmed = payload.trim();
                                    if trimmed.is_empty() {
                                        continue;
                                    }
                                    match wp_core_connectors::sources::batch::ndjson::ndjson_to_record_batch(
                                        &[trimmed.to_string()],
                                        schema.as_ref(),
                                    ) {
                                        Ok(Some(rb)) => {
                                            if let Err(e) = crate::receiver::route_batch(
                                                &stream_name,
                                                rb,
                                                router.as_ref(),
                                                metrics.as_ref(),
                                            ) {
                                                if let Some(metrics) = &metrics {
                                                    metrics.inc_route_error();
                                                }
                                                wf_warn!(conn, kind = %source_kind, stream = %stream_name, error = %e, "external source route error");
                                            }
                                        }
                                        Ok(None) => {}
                                        Err(e) => {
                                            if let Some(metrics) = &metrics {
                                                metrics.inc_receiver_decode_error();
                                            }
                                            wf_warn!(conn, kind = %source_kind, stream = %stream_name, error = %e, "external source decode error");
                                        }
                                    }
                                }
                            }
                            Ok(_) => {}
                            Err(e) => {
                                if consecutive_errors == 0 {
                                    wf_warn!(conn, kind = %source_kind, error = %e, "source receive error, will retry");
                                }
                                consecutive_errors = consecutive_errors.saturating_add(1);
                                let delay = if consecutive_errors <= 1 {
                                    std::time::Duration::from_millis(500)
                                } else {
                                    std::time::Duration::from_secs(5)
                                };
                                tokio::time::sleep(delay).await;
                            }
                        }
                    }
                    _ = cancel.cancelled() => break,
                }
            }
            Ok(())
        }));
        spawned += 1;
    }

    if spawned == 0 {
        return RuntimeReason::Bootstrap
            .to_err()
            .with_detail(format!(
                "source kind {:?} built no readable source handles",
                source_kind
            ))
            .err();
    }

    Ok(spawned)
}

fn source_param_to_json(value: &str) -> serde_json::Value {
    let trimmed = value.trim();
    match trimmed {
        "true" => return serde_json::Value::Bool(true),
        "false" => return serde_json::Value::Bool(false),
        _ => {}
    }
    if let Ok(parsed) = trimmed.parse::<i64>() {
        return serde_json::Value::Number(parsed.into());
    }
    if let Ok(parsed) = trimmed.parse::<f64>()
        && let Some(number) = serde_json::Number::from_f64(parsed)
    {
        return serde_json::Value::Number(number);
    }
    serde_json::Value::String(value.to_string())
}

pub(super) async fn spawn_metrics_task(
    config: &FusionConfig,
    router: &Arc<Router>,
    cancel: CancellationToken,
    metrics: Option<Arc<RuntimeMetrics>>,
    dispatcher: Option<Arc<SinkDispatcher>>,
) -> RuntimeResult<TaskGroup> {
    let mut group = TaskGroup::new("metrics");
    if !config.metrics.enabled {
        return Ok(group);
    }
    let Some(metrics) = metrics else {
        return Ok(group);
    };
    let router_clone = Arc::clone(router);
    let metrics_config = config.metrics.clone();

    // Create monitor channel if dispatcher is available
    let mon_send = match dispatcher {
        Some(ref d) if d.has_monitor_sinks() => {
            let (tx, rx) = mpsc::channel::<Vec<MetricsRecord>>(64);
            let d = Arc::clone(d);
            tokio::spawn(async move {
                run_monitor_consumer(rx, d).await;
            });
            Some(tx)
        }
        _ => None,
    };

    group.push(tokio::spawn(async move {
        run_metrics_task(metrics, metrics_config, router_clone, cancel, mon_send)
            .await
            .source_err(RuntimeReason::system_error(), "run metrics task")?;
        Ok(())
    }));
    Ok(group)
}

async fn run_monitor_consumer(mut rx: MonRecv, dispatcher: Arc<SinkDispatcher>) {
    while let Some(records) = rx.recv().await {
        for record in records {
            let data = metrics_record_to_data_record(&record);
            dispatcher.dispatch_to_monitor(&data).await;
        }
    }
}

fn metrics_record_to_data_record(record: &MetricsRecord) -> DataRecord {
    let mut out = DataRecord::default();
    for (key, value) in &record.fields {
        let field = Field::new(DataType::Chars, key, Value::from(value.as_str()));
        out.push(FieldStorage::from_owned(field));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::source_param_to_json;

    #[test]
    fn source_param_to_json_preserves_connector_types() {
        assert_eq!(source_param_to_json("5514"), serde_json::json!(5514));
        assert_eq!(source_param_to_json("true"), serde_json::json!(true));
        assert_eq!(source_param_to_json("1.5"), serde_json::json!(1.5));
        assert_eq!(
            source_param_to_json("0.0.0.0"),
            serde_json::json!("0.0.0.0")
        );
    }
}
