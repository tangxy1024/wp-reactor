use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orion_error::conversion::{SourceErr, ToStructError};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use wf_config::{FusionConfig, SourceConfig};
use wf_engine::alert::OutputRecord;
use wf_engine::sink::SinkDispatcher;
use wf_engine::window::{Evictor, Router, WindowRegistry};

use crate::alert_task;
use crate::engine_task::{RuleTaskConfig, WindowSource, run_rule_task};
use crate::error::{RuntimeReason, RuntimeResult};
use crate::evictor_task;
use crate::metrics::{MetricsRecord, MonRecv, RuntimeMetrics, run_metrics_task};
use wp_model_core::model::{DataRecord, DataType, Field, FieldStorage, Value};
use crate::receiver::{Receiver, replay_kafka};

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
        match source {
            SourceConfig::Tcp(tcp) => {
                if !tcp.enabled {
                    continue;
                }
                let receiver = Receiver::bind(&tcp.listen, Arc::clone(&router), metrics.clone())
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
            SourceConfig::File(file) => {
                if !file.enabled {
                    continue;
                }
                let path = resolve_source_path(base_dir, &file.path);
                let stream = file.stream.clone();
                let router = Arc::clone(&router);
                let metrics = metrics.clone();
                let cancel = cancel.child_token();
                let _format = file.format;
                let schemas = Arc::clone(&schema_catalog);

                // Resolve schema for the stream
                let arrow_schema = schemas
                    .iter()
                    .find(|s| s.streams.iter().any(|sub| sub == &stream))
                    .map(|ws| {
                        use arrow::datatypes::{DataType, Field, Schema, TimeUnit};
                        use wf_lang::{BaseType, FieldType};
                        let fields: Vec<Field> = ws
                            .fields
                            .iter()
                            .map(|f| {
                                let dt = match &f.field_type {
                                    FieldType::Base(BaseType::Chars) => DataType::Utf8,
                                    FieldType::Base(BaseType::Digit) => DataType::Int64,
                                    FieldType::Base(BaseType::Float) => DataType::Float64,
                                    FieldType::Base(BaseType::Bool) => DataType::Boolean,
                                    FieldType::Base(BaseType::Time) => {
                                        DataType::Timestamp(TimeUnit::Nanosecond, None)
                                    }
                                    FieldType::Base(BaseType::Ip) => DataType::Utf8,
                                    FieldType::Array(_) => DataType::Utf8,
                                    _ => DataType::Utf8,
                                };
                                Field::new(&f.name, dt, true)
                            })
                            .collect();
                        Arc::new(Schema::new(fields))
                    });

                let Some(arrow_schema) = arrow_schema else {
                    wf_warn!(conn, stream = %stream, "no schema found for stream, skipping source");
                    continue;
                };

                group.push(tokio::spawn(async move {
                    let source = wp_core_connectors::sources::batch::file::SimpleFileSource::open(&path)
                        .await
                        .map_err(|e| RuntimeReason::system_error().to_err()
                            .with_detail(format!("open file source {}: {}", path.display(), e)))?;
                    let batch_source = wp_core_connectors::sources::batch::file::FileBatchSource::new(
                        &stream,
                        Box::new(source),
                        arrow_schema,
                    );
                    crate::source::run_batch_source(
                        stream,
                        Box::new(batch_source),
                        router,
                        metrics,
                        cancel,
                    )
                    .await
                }));
                spawned += 1;
            }
            SourceConfig::Kafka(kafka) => {
                if !kafka.enabled {
                    continue;
                }
                let stream = kafka.stream.clone();
                let router = Arc::clone(&router);
                let metrics = metrics.clone();
                let cancel = cancel.child_token();
                let format = kafka.format;
                let brokers = kafka.brokers.clone();
                let topic = kafka.topic.clone();
                let group_id = kafka.group_id.clone();
                let schemas = Arc::clone(&schema_catalog);
                group.push(tokio::spawn(async move {
                    replay_kafka(&brokers, &topic, &group_id, format, &stream, schemas.as_slice(), router, metrics, cancel).await
                }));
                spawned += 1;
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
