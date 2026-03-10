use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orion_error::prelude::*;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use wf_config::{FileInputFormat, FusionConfig, SourceConfig};
use wf_core::alert::OutputRecord;
use wf_core::sink::SinkDispatcher;
use wf_core::window::{Evictor, Router, WindowRegistry};

use crate::alert_task;
use crate::engine_task::{RuleTaskConfig, WindowSource, run_rule_task};
use crate::error::RuntimeResult;
use crate::evictor_task;
use crate::metrics::{RuntimeMetrics, run_metrics_task};
use crate::receiver::{Receiver, replay_ndjson_file};

use super::types::{RunRule, TaskGroup};

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
    schemas: &[wf_lang::WindowSchema],
    alert_tx: mpsc::Sender<OutputRecord>,
    _config: &FusionConfig,
    cancel: CancellationToken,
    metrics: Option<Arc<RuntimeMetrics>>,
) -> TaskGroup {
    let mut group = TaskGroup::new("rules");
    let timeout_scan_interval = Duration::from_secs(1);

    for rule in rules {
        let window_sources =
            resolve_window_sources(&rule.stream_aliases, schemas, router.registry());

        let task_config = RuleTaskConfig {
            machine: rule.machine,
            executor: rule.executor,
            window_sources,
            stream_aliases: rule.stream_aliases,
            alert_tx: alert_tx.clone(),
            cancel: cancel.child_token(),
            timeout_scan_interval,
            router: Arc::clone(router),
            metrics: metrics.clone(),
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

/// Resolve which windows a rule needs to subscribe to, based on its
/// stream_aliases (stream → alias mapping) and the window schemas (which
/// define which streams flow into each window).
pub(super) fn resolve_window_sources(
    stream_aliases: &HashMap<String, Vec<String>>,
    schemas: &[wf_lang::WindowSchema],
    registry: &WindowRegistry,
) -> Vec<WindowSource> {
    // Collect all stream names this engine cares about.
    let interested_streams: std::collections::HashSet<&str> =
        stream_aliases.keys().map(|s| s.as_str()).collect();

    // For each window schema, check if any of its streams match.
    let mut seen_windows = std::collections::HashSet::new();
    let mut sources = Vec::new();

    for ws in schemas {
        if seen_windows.contains(&ws.name) {
            continue;
        }
        let matching_streams: Vec<String> = ws
            .streams
            .iter()
            .filter(|s| interested_streams.contains(s.as_str()))
            .cloned()
            .collect();
        if matching_streams.is_empty() {
            continue;
        }
        if let Some(window) = registry.get_window(&ws.name)
            && let Some(notify) = registry.get_notifier(&ws.name)
        {
            sources.push(WindowSource {
                window_name: ws.name.clone(),
                window: Arc::clone(window),
                notify: Arc::clone(notify),
                stream_names: matching_streams,
            });
            seen_windows.insert(ws.name.clone());
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
) -> RuntimeResult<(SocketAddr, TaskGroup)> {
    let mut group = TaskGroup::new("receiver");
    let mut listen_addr: Option<SocketAddr> = None;
    let mut spawned = 0usize;
    let schema_catalog = Arc::new(schemas.to_vec());

    for source in &config.sources {
        match source {
            SourceConfig::Tcp(tcp) => {
                if !tcp.enabled {
                    continue;
                }
                let receiver = Receiver::bind(&tcp.listen, Arc::clone(&router), metrics.clone())
                    .await
                    .owe_sys()?;
                let bound = receiver.local_addr().owe_sys()?;
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
            SourceConfig::File(file) => {
                if !file.enabled {
                    continue;
                }
                let path = resolve_source_path(base_dir, &file.path);
                let stream = file.stream.clone();
                let router = Arc::clone(&router);
                let metrics = metrics.clone();
                let cancel = cancel.child_token();
                let format = file.format;
                let schemas = Arc::clone(&schema_catalog);
                group.push(tokio::spawn(async move {
                    match format {
                        FileInputFormat::Ndjson => {
                            replay_ndjson_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?;
                        }
                    }
                    Ok(())
                }));
                spawned += 1;
            }
        }
    }

    if spawned == 0 {
        return Err(StructError::from(crate::error::RuntimeReason::Bootstrap)
            .with_detail("no enabled sources configured"));
    }

    Ok((
        listen_addr.unwrap_or(SocketAddr::from(([0, 0, 0, 0], 0))),
        group,
    ))
}

fn resolve_source_path(base_dir: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        base_dir.join(p)
    }
}

pub(super) async fn spawn_metrics_task(
    config: &FusionConfig,
    router: &Arc<Router>,
    cancel: CancellationToken,
    metrics: Option<Arc<RuntimeMetrics>>,
) -> RuntimeResult<TaskGroup> {
    let mut group = TaskGroup::new("metrics");
    if !config.metrics.enabled {
        return Ok(group);
    }
    let Some(metrics) = metrics else {
        return Ok(group);
    };
    let listener = TcpListener::bind(&config.metrics.prometheus_listen)
        .await
        .owe_sys()?;
    let router = Arc::clone(router);
    let metrics_config = config.metrics.clone();
    group.push(tokio::spawn(async move {
        run_metrics_task(metrics, metrics_config, listener, router, cancel).await?;
        Ok(())
    }));
    Ok(group)
}
