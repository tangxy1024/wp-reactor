use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
    Receiver, replay_arrow_framed_file, replay_arrow_ipc_file, replay_csv_file, replay_kafka,
    replay_ndjson_file,
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
            "kafka" => {
                let stream = source.params.get("stream").cloned().unwrap_or_default();
                let router = Arc::clone(&router);
                let metrics = metrics.clone();
                let cancel = cancel.child_token();
                let brokers: Vec<String> = source
                    .params
                    .get("brokers")
                    .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
                    .unwrap_or_default();
                let topic = source.params.get("topic").cloned().unwrap_or_default();
                let group_id = source
                    .params
                    .get("group_id")
                    .cloned()
                    .unwrap_or_else(|| "wfusion".into());
                let schemas = Arc::clone(&schema_catalog);
                group.push(tokio::spawn(async move {
                    replay_kafka(
                        &brokers,
                        &topic,
                        &group_id,
                        &stream,
                        schemas.as_slice(),
                        router,
                        metrics,
                        cancel,
                    )
                    .await
                }));
                spawned += 1;
            }
            _ => {}
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
