use std::sync::Arc;
use std::time::Instant;

use tokio::sync::mpsc;

use wf_engine::alert::OutputRecord;
use wf_engine::sink::SinkDispatcher;

use crate::metrics::RuntimeMetrics;

/// Bounded channel capacity for the alert pipeline.
pub const ALERT_CHANNEL_CAPACITY: usize = 64;

/// Consume alert records from the channel and route them via the connector-based
/// `SinkDispatcher`.
///
/// Shutdown is driven by channel close: when the scheduler finishes
/// its drain + flush and drops its `Sender<OutputRecord>`, `rx.recv()` returns
/// `None` and this task exits. After all records are consumed, all sinks in
/// the dispatcher are gracefully stopped.
pub async fn run_alert_dispatcher(
    mut rx: mpsc::Receiver<OutputRecord>,
    dispatcher: Arc<SinkDispatcher>,
    metrics: Option<Arc<RuntimeMetrics>>,
) {
    while let Some(record) = rx.recv().await {
        let data_record = match record.to_data_record() {
            Ok(data) => data,
            Err(e) => {
                if let Some(metrics) = &metrics {
                    metrics.inc_alert_serialize_failed();
                }
                log::warn!("alert export error: {e}");
                continue;
            }
        };
        let dispatch_started = Instant::now();
        dispatcher
            .dispatch(&record.yield_target, &data_record)
            .await;
        if let Some(metrics) = &metrics {
            metrics.inc_alert_dispatch();
            metrics.observe_alert_dispatch(dispatch_started.elapsed());
        }
    }
    dispatcher.stop_all().await;
}
