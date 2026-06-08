use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orion_error::conversion::{SourceErr, ToStructError};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use wf_config::{FileInputFormat, FusionConfig, SourceConfig};
use wf_engine::alert::OutputRecord;
use wf_engine::sink::SinkDispatcher;
use wf_engine::window::{Evictor, Router, WindowRegistry};

use crate::alert_task;
use crate::engine_task::{RuleTaskConfig, WindowSource, run_rule_task};
use crate::error::{RuntimeReason, RuntimeResult};
use crate::evictor_task;
use crate::metrics::{RuntimeMetrics, run_metrics_task};
use crate::receiver::{
    Receiver, replay_arrow_framed_file, replay_arrow_ipc_file, replay_csv_file, replay_ndjson_file,
};

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
                        FileInputFormat::Csv => {
                            replay_csv_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?;
                        }
                        FileInputFormat::ArrowFramed => {
                            replay_arrow_framed_file(
                                &path,
                                &stream,
                                schemas.as_slice(),
                                router,
                                metrics,
                                cancel,
                            )
                            .await?;
                        }
                        FileInputFormat::ArrowIpc => {
                            replay_arrow_ipc_file(
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
        .source_err(
            RuntimeReason::system_error(),
            "bind prometheus metrics listener",
        )?;
    let router = Arc::clone(router);
    let metrics_config = config.metrics.clone();
    group.push(tokio::spawn(async move {
        run_metrics_task(metrics, metrics_config, listener, router, cancel)
            .await
            .source_err(RuntimeReason::system_error(), "run metrics task")?;
        Ok(())
    }));
    Ok(group)
}
