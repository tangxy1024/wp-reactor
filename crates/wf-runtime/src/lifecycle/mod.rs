mod bootstrap;
mod compile;
mod reload;
mod signal;
mod spawn;
mod types;

use orion_error::conversion::ToStructError;
use orion_error::op_context;
use orion_error::prelude::*;
use std::net::SocketAddr;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use wf_config::FusionConfig;

use crate::error::{RuntimeReason, RuntimeResult};

// Re-export public API
pub use reload::{PreparedRuleReload, ReloadPreparation, prepare_reload};
pub use signal::{ShutdownTrigger, wait_for_signal};

use crate::metrics::maybe_build_metrics;
use bootstrap::load_and_compile;
use spawn::{
    spawn_alert_task, spawn_evictor_task, spawn_metrics_task, spawn_receiver_task, spawn_rule_tasks,
};
use types::TaskGroup;

fn mode_name(mode: wf_config::FusionMode) -> &'static str {
    match mode {
        wf_config::FusionMode::Daemon => "daemon",
        wf_config::FusionMode::Batch => "batch",
    }
}

// ---------------------------------------------------------------------------
// Reactor — the top-level lifecycle handle
// ---------------------------------------------------------------------------

/// Manages the full lifecycle of the CEP runtime: bootstrap, run, and
/// graceful shutdown.
///
/// Task groups are stored in start order and joined in reverse (LIFO)
/// during [`wait`](Self::wait), ensuring correct drain sequencing:
/// receiver stops first, then rule tasks drain and flush, then alert
/// sink flushes to disk, and finally background tasks stop.
pub struct Reactor {
    cancel: CancellationToken,
    watchers: Vec<JoinHandle<RuntimeResult<()>>>,
    listen_addr: Option<SocketAddr>,
}

impl Reactor {
    /// Bootstrap the entire runtime from a [`FusionConfig`] and a base
    /// directory (for resolving relative `.wfs` / `.wfl` file paths).
    #[tracing::instrument(name = "engine.start", skip_all, fields(mode = %mode_name(config.mode)))]
    pub async fn start(config: FusionConfig, base_dir: &std::path::Path) -> RuntimeResult<Self> {
        let mut op = op_context!("engine-bootstrap").with_auto_log();
        op.record("mode", mode_name(config.mode));
        op.record("base_dir", base_dir.display().to_string().as_str());

        let cancel = CancellationToken::new();
        let rule_cancel = CancellationToken::new();

        // Phase 1: Load config & compile rules + build sink dispatcher
        let data = load_and_compile(&config, base_dir).await?;
        wf_info!(
            sys,
            schemas = data.schema_count,
            rules = data.rules.len(),
            "engine bootstrap complete"
        );

        let rule_names: Vec<String> = data
            .rules
            .iter()
            .map(|rule| rule.executor.plan().name.clone())
            .collect();
        let window_names: Vec<String> = data
            .router
            .registry()
            .window_names()
            .map(str::to_string)
            .collect();
        let metrics = maybe_build_metrics(&config.metrics, &rule_names, &window_names);

        // Phase 2: Spawn task groups (start order: alert → evictor → rules → receiver → metrics)
        let mut watchers: Vec<JoinHandle<RuntimeResult<()>>> = Vec::with_capacity(5);

        let (alert_tx, alert_group) = spawn_alert_task(data.dispatcher, metrics.clone());
        watchers.push(watch_group(alert_group, cancel.clone()));

        watchers.push(watch_group(
            spawn_evictor_task(&config, &data.router, cancel.child_token(), metrics.clone()),
            cancel.clone(),
        ));

        let rule_group = spawn_rule_tasks(
            data.rules,
            &data.router,
            &data.intermediate_targets,
            alert_tx,
            rule_cancel.child_token(),
            metrics.clone(),
        );
        watchers.push(watch_group(rule_group, cancel.clone()));

        let (listen_addr, receiver_group) = spawn_receiver_task(
            &config,
            data.router.clone(),
            cancel.clone(),
            metrics.clone(),
            &data.schemas,
            base_dir,
        )
        .await?;
        watchers.push(watch_receiver_group(
            receiver_group,
            cancel.clone(),
            rule_cancel.clone(),
            config.mode == wf_config::FusionMode::Batch,
        ));
        watchers.push(watch_group(
            spawn_metrics_task(&config, &data.router, cancel.child_token(), metrics).await?,
            cancel.clone(),
        ));

        op.mark_suc();
        Ok(Self {
            cancel,
            watchers,
            listen_addr,
        })
    }

    /// Returns the local address the engine is listening on.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.listen_addr
    }

    /// Request graceful shutdown of all tasks.
    pub fn shutdown(&self) {
        wf_info!(sys, "initiating graceful shutdown");
        self.cancel.cancel();
    }

    /// Wait for all task groups to complete after shutdown.
    pub async fn wait(mut self) -> RuntimeResult<()> {
        let mut first_error: Option<StructError<RuntimeReason>> = None;
        while let Some(handle) = self.watchers.pop() {
            let result = handle.await.map_err(|e| {
                RuntimeReason::Shutdown
                    .to_err()
                    .with_detail(format!("supervisor join error: {e}"))
            })?;
            if let Err(err) = result
                && first_error.is_none()
            {
                first_error = Some(
                    Err::<(), _>(err)
                        .source_err(RuntimeReason::Shutdown, "supervisor failed")
                        .unwrap_err(),
                );
            }
        }
        if let Some(err) = first_error {
            return Err(err);
        }
        Ok(())
    }

    /// Returns a clone of the root cancellation token (for signal integration).
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

fn watch_group(group: TaskGroup, cancel: CancellationToken) -> JoinHandle<RuntimeResult<()>> {
    let name = group.name;
    tokio::spawn(async move {
        wf_debug!(sys, task_group = name, "watching task group");
        let result = group.wait().await;
        if result.is_err() && !cancel.is_cancelled() {
            cancel.cancel();
        }
        result?;
        wf_debug!(sys, task_group = name, "task group finished");
        Ok(())
    })
}

fn watch_receiver_group(
    receiver_group: TaskGroup,
    cancel: CancellationToken,
    rule_cancel: CancellationToken,
    auto_shutdown: bool,
) -> JoinHandle<RuntimeResult<()>> {
    let name = receiver_group.name;
    tokio::spawn(async move {
        wf_debug!(sys, task_group = name, "watching task group");
        let result = receiver_group.wait().await;
        rule_cancel.cancel();
        if result.is_err() && !cancel.is_cancelled() {
            cancel.cancel();
        } else if auto_shutdown && result.is_ok() && !cancel.is_cancelled() {
            wf_info!(
                sys,
                task_group = name,
                "batch receiver completed; initiating automatic shutdown"
            );
            cancel.cancel();
        }
        result?;
        wf_debug!(sys, task_group = name, "task group finished");
        Ok(())
    })
}
