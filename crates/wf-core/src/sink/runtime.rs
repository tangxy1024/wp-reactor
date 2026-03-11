use tokio::sync::Mutex;
use wp_connector_api::{SinkHandle, SinkSpec as ResolvedSinkSpec};
use wp_model_core::model::DataRecord;

/// Runtime state for a single sink instance.
///
/// Wraps a `SinkHandle` (from wp-connector-api) with metadata and provides
/// convenience methods for sending alert JSON data and lifecycle management.
pub struct SinkRuntime {
    pub name: String,
    pub spec: ResolvedSinkSpec,
    pub handle: Mutex<SinkHandle>,
    pub tags: Vec<String>,
}

impl SinkRuntime {
    /// Send raw string payloads via `AsyncRawDataSink::sink_str`.
    pub async fn send_str(&self, data: &str) -> anyhow::Result<()> {
        let mut handle = self.handle.lock().await;
        handle
            .sink
            .sink_str(data)
            .await
            .map_err(|e| anyhow::anyhow!("sink {:?} send error: {e}", self.name))
    }

    /// Send structured records via `AsyncRecordSink::sink_record`.
    pub async fn send_record(&self, data: &DataRecord) -> anyhow::Result<()> {
        let mut handle = self.handle.lock().await;
        handle
            .sink
            .sink_record(data)
            .await
            .map_err(|e| anyhow::anyhow!("sink {:?} send error: {e}", self.name))
    }

    /// Gracefully stop the sink.
    pub async fn stop(&self) -> anyhow::Result<()> {
        let mut handle = self.handle.lock().await;
        handle
            .sink
            .stop()
            .await
            .map_err(|e| anyhow::anyhow!("sink {:?} stop error: {e}", self.name))
    }
}

impl std::fmt::Debug for SinkRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinkRuntime")
            .field("name", &self.name)
            .field("spec", &self.spec)
            .field("tags", &self.tags)
            .finish_non_exhaustive()
    }
}
