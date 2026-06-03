use orion_error::conversion::{SourceErr, ToStructError};
use tokio::sync::Mutex;
use wp_connector_api::{SinkHandle, SinkSpec as ResolvedSinkSpec};
use wp_model_core::model::DataRecord;

use crate::error::{CoreReason, CoreResult};

/// Runtime state for a single sink instance.
///
/// Wraps a `SinkHandle` (from wp-connector-api) with metadata and provides
/// convenience methods for sending alert JSON data and lifecycle management.
#[derive(::moju_derive::MoJu)]
#[moju(kind = "struct", domain = "Engine", module = "Engine.SinkDispatch")]
pub struct SinkRuntime {
    pub name: String,
    pub spec: ResolvedSinkSpec,
    pub handle: Mutex<SinkHandle>,
    pub tags: Vec<String>,
    pub output_fields: Option<Vec<String>>,
}

impl SinkRuntime {
    /// Send raw string payloads via `AsyncRawDataSink::sink_str`.
    pub async fn send_str(&self, data: &str) -> CoreResult<()> {
        let mut handle = self.handle.lock().await;
        handle.sink.sink_str(data).await.source_err(
            CoreReason::Sink,
            format!("sink {:?} send string", self.name),
        )
    }

    /// Send structured records via `AsyncRecordSink::sink_record`.
    pub async fn send_record(&self, data: &DataRecord) -> CoreResult<()> {
        let projected;
        let data = if let Some(fields) = &self.output_fields {
            projected = project_record(data, fields)?;
            &projected
        } else {
            data
        };
        let mut handle = self.handle.lock().await;
        handle.sink.sink_record(data).await.source_err(
            CoreReason::Sink,
            format!("sink {:?} send record", self.name),
        )
    }

    /// Gracefully stop the sink.
    pub async fn stop(&self) -> CoreResult<()> {
        let mut handle = self.handle.lock().await;
        handle
            .sink
            .stop()
            .await
            .source_err(CoreReason::Sink, format!("sink {:?} stop", self.name))
    }
}

impl std::fmt::Debug for SinkRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SinkRuntime")
            .field("name", &self.name)
            .field("spec", &self.spec)
            .field("tags", &self.tags)
            .field("output_fields", &self.output_fields)
            .finish_non_exhaustive()
    }
}

fn project_record(data: &DataRecord, fields: &[String]) -> CoreResult<DataRecord> {
    let mut record = DataRecord::default();
    for name in fields {
        let Some(field) = data.field(name) else {
            return CoreReason::Sink
                .to_err()
                .with_detail(format!("sink requested missing output field {:?}", name))
                .err();
        };
        record.push(field.clone());
    }
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wp_model_core::model::{DataType, Field, FieldStorage, Value};

    #[test]
    fn project_record_filters_and_reorders_fields() {
        let mut record = DataRecord::default();
        record.push(FieldStorage::from_owned(Field::new(
            DataType::Chars,
            "a",
            Value::from("va"),
        )));
        record.push(FieldStorage::from_owned(Field::new(
            DataType::Chars,
            "b",
            Value::from("vb"),
        )));
        record.push(FieldStorage::from_owned(Field::new(
            DataType::Chars,
            "c",
            Value::from("vc"),
        )));

        let projected = project_record(&record, &["c".to_string(), "a".to_string()]).unwrap();

        assert_eq!(projected.items.len(), 2);
        assert_eq!(projected.items[0].get_name(), "c");
        assert_eq!(projected.items[1].get_name(), "a");
    }

    #[test]
    fn project_record_rejects_missing_field() {
        let mut record = DataRecord::default();
        record.push(FieldStorage::from_owned(Field::new(
            DataType::Chars,
            "a",
            Value::from("va"),
        )));

        let err = project_record(&record, &["missing".to_string()]).unwrap_err();
        assert!(err.to_string().contains("missing output field"));
    }
}
