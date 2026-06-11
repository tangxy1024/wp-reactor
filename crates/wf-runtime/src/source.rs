//! Unified source consumer — feeds `BatchSource` output into the engine router.

use std::sync::Arc;

use orion_error::conversion::ToStructError;
use tokio_util::sync::CancellationToken;
use wf_connector_api::BatchSource;
use wf_engine::window::Router;

use crate::error::{RuntimeReason, RuntimeResult};
use crate::metrics::RuntimeMetrics;

/// Run a `BatchSource`→Router consume loop until EOF or cancellation.
///
/// For each `receive_batch()` call, every `RecordBatch` is routed into the
/// engine via `router.route(stream_name, batch)`.
pub async fn run_batch_source(
    stream_name: String,
    mut source: Box<dyn BatchSource>,
    router: Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
) -> RuntimeResult<()> {
    source.start().await.map_err(|e| {
        RuntimeReason::Bootstrap
            .to_err()
            .with_detail(format!("source {} start: {}", source.identifier(), e))
    })?;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = source.receive_batch() => {
                let batches = match result {
                    Ok(b) => b,
                    Err(e) if matches!(e.reason(), wf_connector_api::SourceReason::EOF) => break,
                    Err(e) => {
                        return Err(RuntimeReason::system_error().to_err()
                            .with_detail(format!("source {} receive: {}", source.identifier(), e)));
                    }
                };

                for batch in batches {
                    if let Some(metrics) = &metrics {
                        metrics.add_receiver_frame(batch.num_rows());
                        metrics.inc_router_route_call();
                    }
                    let report = router.route(&stream_name, batch)
                        .map_err(|e| RuntimeReason::system_error().to_err()
                            .with_detail(format!("route batch: {}", e)))?;
                    if let Some(metrics) = &metrics {
                        metrics.add_route_report(&report);
                    }
                }
            }
        }
    }

    source.close().await.map_err(|e| {
        RuntimeReason::system_error()
            .to_err()
            .with_detail(format!("source {} close: {}", source.identifier(), e))
    })?;

    Ok(())
}
