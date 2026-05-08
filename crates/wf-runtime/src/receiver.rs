use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use arrow::array::{
    ArrayRef, BooleanArray, Float64Array, Int64Array, StringArray, TimestampNanosecondArray,
};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use arrow::ipc::reader::FileReader;
use arrow::record_batch::RecordBatch;
use orion_error::conversion::{ConvErr, SourceErr, SourceRawErr, ToStructError};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;
use wf_core::window::Router;
use wf_lang::{BaseType, FieldType, WindowSchema};

use crate::error::{RuntimeReason, RuntimeResult};
use crate::metrics::RuntimeMetrics;

/// TCP receiver that accepts connections, reads length-prefixed Arrow IPC
/// frames, decodes them, and routes batches to the [`Router`].
pub struct Receiver {
    listener: TcpListener,
    router: Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
}

impl Receiver {
    /// Parse `"tcp://host:port"` and bind a TCP listener.
    pub async fn bind(
        listen: &str,
        router: Arc<Router>,
        metrics: Option<Arc<RuntimeMetrics>>,
    ) -> RuntimeResult<Self> {
        let addr = listen.strip_prefix("tcp://").unwrap_or(listen);
        let listener = TcpListener::bind(addr).await.source_err(
            RuntimeReason::system_error(),
            format!("bind tcp listener {addr}"),
        )?;
        Ok(Self {
            listener,
            router,
            metrics,
            cancel: CancellationToken::new(),
        })
    }

    /// Returns the local address the listener is bound to.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Returns a clone of the cancellation token for external shutdown signaling.
    pub fn cancel_token(&self) -> CancellationToken {
        self.cancel.clone()
    }

    /// Start the accept loop. Blocks until the cancellation token is triggered.
    #[tracing::instrument(name = "receiver", skip_all)]
    pub async fn run(self) -> RuntimeResult<()> {
        loop {
            tokio::select! {
                result = self.listener.accept() => {
                    let (stream, peer) = result
                        .source_err(RuntimeReason::system_error(), "accept tcp receiver connection")?;
                    wf_debug!(conn, peer = %peer, "accepted connection");
                    if let Some(metrics) = &self.metrics {
                        metrics.inc_receiver_connection();
                    }
                    let router = Arc::clone(&self.router);
                    let metrics = self.metrics.clone();
                    let cancel = self.cancel.child_token();
                    tokio::spawn(handle_connection(stream, router, metrics, cancel, peer));
                }
                _ = self.cancel.cancelled() => break,
            }
        }
        Ok(())
    }
}

#[tracing::instrument(skip_all, fields(peer = %peer))]
async fn handle_connection(
    stream: TcpStream,
    router: Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
    peer: SocketAddr,
) {
    let (reader, _writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    loop {
        tokio::select! {
            result = read_frame(&mut reader) => {
                match result {
                    Ok(None) => break,
                    Ok(Some(payload)) => {
                        let decode_started = Instant::now();
                        match wp_arrow::ipc::decode_ipc(&payload) {
                            Ok(frame) => {
                                if let Some(metrics) = &metrics {
                                    metrics.observe_receiver_decode(decode_started.elapsed());
                                }
                                match route_batch(&frame.tag, frame.batch, router.as_ref(), metrics.as_ref()) {
                                    Ok(()) => {}
                                    Err(e) => {
                                        if let Some(metrics) = &metrics {
                                            metrics.inc_route_error();
                                        }
                                        wf_warn!(pipe, error = %e, "route error");
                                    }
                                }
                            }
                            Err(e) => {
                                if let Some(metrics) = &metrics {
                                    metrics.observe_receiver_decode(decode_started.elapsed());
                                }
                                if let Some(metrics) = &metrics {
                                    metrics.inc_receiver_decode_error();
                                }
                                wf_warn!(conn, error = %e, "IPC decode error")
                            }
                        }
                    }
                    Err(e) => {
                        if let Some(metrics) = &metrics {
                            metrics.inc_receiver_read_error();
                        }
                        wf_warn!(conn, error = %e, "connection read error");
                        break;
                    }
                }
            }
            _ = cancel.cancelled() => break,
        }
    }
    wf_debug!(conn, peer = %peer, "connection closed");
}

/// Replay NDJSON events from file and route them into the runtime as one
/// configured stream.
///
/// Each line must be a JSON object whose field names match the subscribed
/// window schema for `stream_name`.
pub async fn replay_ndjson_file(
    path: &Path,
    stream_name: &str,
    schemas: &[WindowSchema],
    router: Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
) -> RuntimeResult<()> {
    const FILE_BATCH_ROWS: usize = 2048;

    let schema = resolve_stream_schema(schemas, stream_name)?;
    let file = tokio::fs::File::open(path).await.source_err(
        RuntimeReason::system_error(),
        format!("open file source {}", path.display()),
    )?;
    let mut lines = BufReader::new(file).lines();
    let mut rows: Vec<serde_json::Map<String, serde_json::Value>> =
        Vec::with_capacity(FILE_BATCH_ROWS);
    let mut line_no = 0usize;
    let mut total_rows = 0usize;

    wf_info!(
        conn,
        source = %path.display(),
        stream = stream_name,
        "starting file source replay"
    );
    if let Some(metrics) = &metrics {
        metrics.inc_receiver_connection();
    }

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            next = lines.next_line() => {
                let Some(line) = next
                    .source_err(RuntimeReason::system_error(), format!("read file source {}", path.display()))?
                else { break };
                line_no += 1;
                if line.trim().is_empty() {
                    continue;
                }
                let value: serde_json::Value = serde_json::from_str(&line).source_err(
                    RuntimeReason::data_error(),
                    format!("invalid NDJSON at {}:{}", path.display(), line_no),
                )?;
                let Some(obj) = value.as_object() else {
                    return RuntimeReason::data_error()
                        .to_err()
                        .with_detail(format!(
                            "invalid NDJSON at {}:{}: expected JSON object",
                            path.display(),
                            line_no
                        ))
                        .err();
                };
                rows.push(obj.clone());
                if rows.len() >= FILE_BATCH_ROWS {
                    let batch = build_record_batch_from_json(&schema, &rows)?;
                    total_rows += batch.num_rows();
                    if let Err(e) = route_batch(stream_name, batch, router.as_ref(), metrics.as_ref()) {
                        if let Some(metrics) = &metrics {
                            metrics.inc_route_error();
                        }
                        return Err(e);
                    }
                    rows.clear();
                }
            }
        }
    }

    if !rows.is_empty() {
        let batch = build_record_batch_from_json(&schema, &rows)?;
        total_rows += batch.num_rows();
        if let Err(e) = route_batch(stream_name, batch, router.as_ref(), metrics.as_ref()) {
            if let Some(metrics) = &metrics {
                metrics.inc_route_error();
            }
            return Err(e);
        }
    }

    wf_info!(
        conn,
        source = %path.display(),
        stream = stream_name,
        rows = total_rows,
        "file source replay complete"
    );
    Ok(())
}

/// Replay framed `wp_arrow` IPC records from file and route them into the
/// runtime.
pub async fn replay_arrow_framed_file(
    path: &Path,
    stream_name: &str,
    schemas: &[WindowSchema],
    router: Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
) -> RuntimeResult<()> {
    let path = path.to_path_buf();
    let stream_override = (!stream_name.trim().is_empty()).then(|| stream_name.to_string());

    wf_info!(
        conn,
        source = %path.display(),
        stream = stream_name,
        "starting arrow file replay"
    );
    if let Some(metrics) = &metrics {
        metrics.inc_receiver_connection();
    }

    let mut file = tokio::fs::File::open(&path).await.source_err(
        RuntimeReason::system_error(),
        format!("open arrow source {}", path.display()),
    )?;
    let mut total_rows = 0usize;

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            next = read_frame(&mut file) => {
                let Some(payload) = next.source_err(
                    RuntimeReason::system_error(),
                    format!("read arrow frame from {}", path.display()),
                )? else {
                    break;
                };

                let frame = wp_arrow::ipc::decode_ipc(&payload)
                    .source_raw_err(
                        RuntimeReason::data_error(),
                        format!("decode arrow frame from {}", path.display()),
                    )?;
                let stream = stream_override.as_deref().unwrap_or(frame.tag.as_str());
                validate_batch_schema_for_stream(schemas, stream, frame.batch.schema().as_ref())?;

                total_rows += frame.batch.num_rows();
                if let Err(e) = route_batch(stream, frame.batch, router.as_ref(), metrics.as_ref()) {
                    if let Some(metrics) = &metrics {
                        metrics.inc_route_error();
                    }
                    return Err(e);
                }
            }
        }
    }

    wf_info!(
        conn,
        source = %path.display(),
        stream = stream_name,
        rows = total_rows,
        "arrow file replay complete"
    );
    Ok(())
}

/// Replay standard Arrow IPC file batches and route them into the runtime as
/// one configured stream.
pub async fn replay_arrow_ipc_file(
    path: &Path,
    stream_name: &str,
    schemas: &[WindowSchema],
    router: Arc<Router>,
    metrics: Option<Arc<RuntimeMetrics>>,
    cancel: CancellationToken,
) -> RuntimeResult<()> {
    let path = path.to_path_buf();
    let stream_name = stream_name.to_string();
    let expected_schema = resolve_stream_schema(schemas, &stream_name)?;

    wf_info!(
        conn,
        source = %path.display(),
        stream = stream_name,
        "starting arrow ipc file replay"
    );
    if let Some(metrics) = &metrics {
        metrics.inc_receiver_connection();
    }

    let path_for_read = path.clone();
    let stream_for_read = stream_name.clone();
    let routed_rows = tokio::task::spawn_blocking(move || -> RuntimeResult<usize> {
        let file = std::fs::File::open(&path_for_read).source_err(
            RuntimeReason::system_error(),
            format!("open arrow ipc source {}", path_for_read.display()),
        )?;
        let mut reader = FileReader::try_new(file, None).source_raw_err(
            RuntimeReason::data_error(),
            format!("read arrow ipc source {}", path_for_read.display()),
        )?;

        let file_schema = reader.schema();
        if file_schema.as_ref() != expected_schema.as_ref() {
            return RuntimeReason::data_error()
                .to_err()
                .with_detail(format!(
                    "arrow ipc source {} schema mismatch for stream {:?}",
                    path_for_read.display(),
                    stream_for_read
                ))
                .err();
        }

        let mut total_rows = 0usize;
        for batch in &mut reader {
            if cancel.is_cancelled() {
                break;
            }
            let batch = batch.source_raw_err(
                RuntimeReason::data_error(),
                format!("read arrow ipc batch from {}", path_for_read.display()),
            )?;
            total_rows += batch.num_rows();
            if let Err(e) = route_batch(&stream_for_read, batch, router.as_ref(), metrics.as_ref())
            {
                if let Some(metrics) = &metrics {
                    metrics.inc_route_error();
                }
                return Err(e);
            }
        }
        Ok(total_rows)
    })
    .await
    .source_raw_err(RuntimeReason::system_error(), "join arrow ipc replay task")??;

    wf_info!(
        conn,
        source = %path.display(),
        stream = stream_name,
        rows = routed_rows,
        "arrow ipc file replay complete"
    );
    Ok(())
}

fn validate_batch_schema_for_stream(
    schemas: &[WindowSchema],
    stream_name: &str,
    batch_schema: &Schema,
) -> RuntimeResult<()> {
    let expected = resolve_stream_schema(schemas, stream_name)?;
    if expected.as_ref() != batch_schema {
        return RuntimeReason::data_error()
            .to_err()
            .with_detail(format!(
                "arrow source schema mismatch for stream {:?}",
                stream_name
            ))
            .err();
    }
    Ok(())
}

fn resolve_stream_schema(schemas: &[WindowSchema], stream_name: &str) -> RuntimeResult<SchemaRef> {
    let mut schema: Option<SchemaRef> = None;
    for ws in schemas {
        if !ws.streams.iter().any(|s| s == stream_name) {
            continue;
        }
        let candidate = window_schema_to_arrow(ws)?;
        if let Some(existing) = &schema {
            if existing.as_ref() != candidate.as_ref() {
                return RuntimeReason::data_error()
                    .to_err()
                    .with_detail(format!(
                        "stream {:?} maps to inconsistent schemas (window {:?})",
                        stream_name, ws.name
                    ))
                    .err();
            }
        } else {
            schema = Some(candidate);
        }
    }
    schema.ok_or_else(|| {
        RuntimeReason::data_error()
            .to_err()
            .with_detail(format!("no schema subscribed for stream {:?}", stream_name))
    })
}

fn window_schema_to_arrow(ws: &WindowSchema) -> RuntimeResult<SchemaRef> {
    let mut fields = Vec::with_capacity(ws.fields.len());
    for field in &ws.fields {
        fields.push(Field::new(
            &field.name,
            field_type_to_arrow(&field.field_type),
            true,
        ));
    }
    Ok(Arc::new(Schema::new(fields)))
}

fn field_type_to_arrow(ft: &FieldType) -> DataType {
    match ft {
        FieldType::Base(base) => base_type_to_arrow(base),
        FieldType::Array(base) => {
            DataType::List(Arc::new(Field::new("item", base_type_to_arrow(base), true)))
        }
    }
}

fn base_type_to_arrow(base: &BaseType) -> DataType {
    match base {
        BaseType::Chars | BaseType::Ip | BaseType::Hex => DataType::Utf8,
        BaseType::Digit => DataType::Int64,
        BaseType::Float => DataType::Float64,
        BaseType::Bool => DataType::Boolean,
        BaseType::Time => DataType::Timestamp(TimeUnit::Nanosecond, None),
    }
}

fn route_batch(
    stream_name: &str,
    batch: RecordBatch,
    router: &Router,
    metrics: Option<&Arc<RuntimeMetrics>>,
) -> RuntimeResult<()> {
    if let Some(metrics) = metrics {
        metrics.add_receiver_frame(batch.num_rows());
        metrics.inc_router_route_call();
    }
    wf_debug!(
        pipe,
        stream = stream_name,
        rows = batch.num_rows(),
        "frame decoded"
    );
    let report = router.route(stream_name, batch).conv_err()?;
    if let Some(metrics) = metrics {
        metrics.add_route_report(&report);
    }
    wf_debug!(
        pipe,
        delivered = report.delivered,
        dropped_late = report.dropped_late,
        skipped = report.skipped_non_local,
        "route report"
    );
    Ok(())
}

fn build_record_batch_from_json(
    schema: &SchemaRef,
    rows: &[serde_json::Map<String, serde_json::Value>],
) -> RuntimeResult<RecordBatch> {
    let mut builders: Vec<ColumnBuilder> = schema
        .fields()
        .iter()
        .map(|f| ColumnBuilder::new(f.data_type(), rows.len()))
        .collect::<RuntimeResult<Vec<_>>>()?;
    for row in rows {
        for (idx, field) in schema.fields().iter().enumerate() {
            builders[idx].push(row.get(field.name()))?;
        }
    }
    let columns: Vec<ArrayRef> = builders.into_iter().map(ColumnBuilder::finish).collect();
    RecordBatch::try_new(schema.clone(), columns).source_raw_err(
        RuntimeReason::data_error(),
        "build file source record batch",
    )
}

enum ColumnBuilder {
    Utf8(Vec<Option<String>>),
    Int64(Vec<Option<i64>>),
    Float64(Vec<Option<f64>>),
    Bool(Vec<Option<bool>>),
    TimeNanos(Vec<Option<i64>>),
}

impl ColumnBuilder {
    fn new(data_type: &DataType, cap: usize) -> RuntimeResult<Self> {
        Ok(match data_type {
            DataType::Utf8 => Self::Utf8(Vec::with_capacity(cap)),
            DataType::Int64 => Self::Int64(Vec::with_capacity(cap)),
            DataType::Float64 => Self::Float64(Vec::with_capacity(cap)),
            DataType::Boolean => Self::Bool(Vec::with_capacity(cap)),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                Self::TimeNanos(Vec::with_capacity(cap))
            }
            _ => {
                return RuntimeReason::data_error()
                    .to_err()
                    .with_detail(format!("unsupported file-source field type: {data_type:?}"))
                    .err();
            }
        })
    }

    fn push(&mut self, value: Option<&serde_json::Value>) -> RuntimeResult<()> {
        match self {
            Self::Utf8(col) => col.push(parse_utf8(value)),
            Self::Int64(col) => col.push(parse_i64(value)),
            Self::Float64(col) => col.push(parse_f64(value)),
            Self::Bool(col) => col.push(parse_bool(value)),
            Self::TimeNanos(col) => col.push(parse_i64(value)),
        }
        Ok(())
    }

    fn finish(self) -> ArrayRef {
        match self {
            Self::Utf8(col) => Arc::new(StringArray::from(col)),
            Self::Int64(col) => Arc::new(Int64Array::from(col)),
            Self::Float64(col) => Arc::new(Float64Array::from(col)),
            Self::Bool(col) => Arc::new(BooleanArray::from(col)),
            Self::TimeNanos(col) => Arc::new(TimestampNanosecondArray::from(col)),
        }
    }
}

fn parse_utf8(v: Option<&serde_json::Value>) -> Option<String> {
    let v = v?;
    match v {
        serde_json::Value::Null => None,
        serde_json::Value::String(s) => Some(s.clone()),
        _ => Some(v.to_string()),
    }
}

fn parse_i64(v: Option<&serde_json::Value>) -> Option<i64> {
    let v = v?;
    match v {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.parse::<i64>().ok(),
        _ => None,
    }
}

fn parse_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    let v = v?;
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.parse::<f64>().ok(),
        _ => None,
    }
}

fn parse_bool(v: Option<&serde_json::Value>) -> Option<bool> {
    let v = v?;
    match v {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" => Some(true),
            "false" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// Read a single length-prefixed frame: `[4B BE u32 len][payload]`.
///
/// Returns `Ok(None)` on clean EOF (connection closed).
async fn read_frame(reader: &mut (impl AsyncReadExt + Unpin)) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let frame_len = u32::from_be_bytes(len_buf) as usize;
    let mut payload = vec![0u8; frame_len];
    reader.read_exact(&mut payload).await?;
    Ok(Some(payload))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Int64Array, TimestampNanosecondArray};
    use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
    use arrow::ipc::writer::FileWriter;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpStream;
    use wf_config::{DistMode, EvictPolicy, LatePolicy, WindowConfig};
    use wf_core::window::{WindowDef, WindowParams, WindowRegistry};

    fn test_schema() -> SchemaRef {
        Arc::new(Schema::new(vec![
            Field::new("ts", DataType::Timestamp(TimeUnit::Nanosecond, None), true),
            Field::new("value", DataType::Int64, true),
        ]))
    }

    fn make_batch(
        schema: &SchemaRef,
        times: &[i64],
        values: &[i64],
    ) -> arrow::record_batch::RecordBatch {
        arrow::record_batch::RecordBatch::try_new(
            schema.clone(),
            vec![
                Arc::new(TimestampNanosecondArray::from(times.to_vec())),
                Arc::new(Int64Array::from(values.to_vec())),
            ],
        )
        .unwrap()
    }

    fn test_config() -> WindowConfig {
        WindowConfig {
            name: "default".into(),
            mode: DistMode::Local,
            max_window_bytes: usize::MAX.into(),
            over_cap: Duration::from_secs(3600).into(),
            evict_policy: EvictPolicy::TimeFirst,
            watermark: Duration::from_secs(0).into(),
            allowed_lateness: Duration::from_secs(3600).into(),
            late_policy: LatePolicy::Drop,
        }
    }

    fn make_router(stream_name: &str) -> Arc<Router> {
        let reg = WindowRegistry::build(vec![WindowDef {
            params: WindowParams {
                name: "test_win".into(),
                schema: test_schema(),
                time_col_index: Some(0),
                over: Duration::from_secs(3600),
            },
            streams: vec![stream_name.to_string()],
            config: test_config(),
        }])
        .unwrap();
        Arc::new(Router::new(reg))
    }

    /// Encode a RecordBatch and wrap it in a length-prefixed outer frame.
    fn make_frame(stream_name: &str, batch: &arrow::record_batch::RecordBatch) -> Vec<u8> {
        let payload = wp_arrow::ipc::encode_ipc(stream_name, batch).unwrap();
        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        frame
    }

    async fn send_frame(stream: &mut TcpStream, frame: &[u8]) {
        stream.write_all(frame).await.unwrap();
        stream.flush().await.unwrap();
    }

    /// Count total rows across all batches in the test window snapshot.
    fn snapshot_row_count(router: &Router) -> usize {
        router
            .registry()
            .snapshot("test_win")
            .unwrap_or_default()
            .iter()
            .map(|b| b.num_rows())
            .sum()
    }

    // -- Test 1: multi_connection_concurrent -----------------------------------

    #[tokio::test]
    async fn multi_connection_concurrent() {
        let router = make_router("events");
        let receiver = Receiver::bind("tcp://127.0.0.1:0", Arc::clone(&router), None)
            .await
            .unwrap();
        let addr = receiver.local_addr().unwrap();
        let cancel = receiver.cancel_token();

        let server = tokio::spawn(async move { receiver.run().await });

        let schema = test_schema();
        let mut handles = Vec::new();
        for i in 0..3 {
            let schema = schema.clone();
            handles.push(tokio::spawn(async move {
                let mut conn = TcpStream::connect(addr).await.unwrap();
                let ts = (i + 1) * 10_000_000_000_i64;
                let batch = make_batch(&schema, &[ts], &[i]);
                let frame = make_frame("events", &batch);
                send_frame(&mut conn, &frame).await;
                // Small delay to ensure the frame is processed before we drop
                tokio::time::sleep(Duration::from_millis(50)).await;
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // Allow processing time
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(snapshot_row_count(&router), 3);

        cancel.cancel();
        server.await.unwrap().unwrap();
    }

    // -- Test 2: continuous_reception ------------------------------------------

    #[tokio::test]
    async fn continuous_reception() {
        let router = make_router("stream");
        let receiver = Receiver::bind("tcp://127.0.0.1:0", Arc::clone(&router), None)
            .await
            .unwrap();
        let addr = receiver.local_addr().unwrap();
        let cancel = receiver.cancel_token();

        let server = tokio::spawn(async move { receiver.run().await });

        let schema = test_schema();
        let mut conn = TcpStream::connect(addr).await.unwrap();
        for i in 0..10 {
            let ts = (i + 1) * 10_000_000_000_i64;
            let batch = make_batch(&schema, &[ts], &[i]);
            let frame = make_frame("stream", &batch);
            send_frame(&mut conn, &frame).await;
        }

        // Allow processing time
        tokio::time::sleep(Duration::from_millis(200)).await;

        assert_eq!(snapshot_row_count(&router), 10);

        cancel.cancel();
        server.await.unwrap().unwrap();
    }

    // -- Test 3: connection_drop_no_impact -------------------------------------

    #[tokio::test]
    async fn connection_drop_no_impact() {
        let router = make_router("data");
        let receiver = Receiver::bind("tcp://127.0.0.1:0", Arc::clone(&router), None)
            .await
            .unwrap();
        let addr = receiver.local_addr().unwrap();
        let cancel = receiver.cancel_token();

        let server = tokio::spawn(async move { receiver.run().await });

        let schema = test_schema();

        // conn_a: send 1 frame then drop
        {
            let mut conn_a = TcpStream::connect(addr).await.unwrap();
            let batch = make_batch(&schema, &[10_000_000_000], &[1]);
            let frame = make_frame("data", &batch);
            send_frame(&mut conn_a, &frame).await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            // conn_a dropped here
        }

        tokio::time::sleep(Duration::from_millis(50)).await;

        // conn_b: send 1 frame after conn_a is gone
        let mut conn_b = TcpStream::connect(addr).await.unwrap();
        let batch = make_batch(&schema, &[20_000_000_000], &[2]);
        let frame = make_frame("data", &batch);
        send_frame(&mut conn_b, &frame).await;

        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(snapshot_row_count(&router), 2);

        cancel.cancel();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn file_ndjson_replay_routes_rows() {
        let router = make_router("events");
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("events.ndjson");
        std::fs::write(
            &file_path,
            r#"{"ts":1000000000,"value":1}
{"ts":"2000000000","value":"2"}
"#,
        )
        .unwrap();

        replay_ndjson_file(
            &file_path,
            "events",
            &[wf_lang::WindowSchema {
                name: "test_win".to_string(),
                streams: vec!["events".to_string()],
                time_field: Some("ts".to_string()),
                over: Duration::from_secs(3600),
                fields: vec![
                    wf_lang::FieldDef {
                        name: "ts".to_string(),
                        field_type: wf_lang::FieldType::Base(wf_lang::BaseType::Time),
                    },
                    wf_lang::FieldDef {
                        name: "value".to_string(),
                        field_type: wf_lang::FieldType::Base(wf_lang::BaseType::Digit),
                    },
                ],
            }],
            Arc::clone(&router),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(snapshot_row_count(&router), 2);
    }

    #[tokio::test]
    async fn file_arrow_framed_replay_routes_rows() {
        let router = make_router("events");
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("events.arrow_framed");
        let schema = test_schema();
        let batch_a = make_batch(&schema, &[1_000_000_000], &[1]);
        let batch_b = make_batch(&schema, &[2_000_000_000], &[2]);

        {
            let payload_a = wp_arrow::ipc::encode_ipc("events", &batch_a).unwrap();
            let payload_b = wp_arrow::ipc::encode_ipc("events", &batch_b).unwrap();
            let mut body = Vec::new();
            body.extend_from_slice(&(payload_a.len() as u32).to_be_bytes());
            body.extend_from_slice(&payload_a);
            body.extend_from_slice(&(payload_b.len() as u32).to_be_bytes());
            body.extend_from_slice(&payload_b);
            std::fs::write(&file_path, body).unwrap();
        }

        replay_arrow_framed_file(
            &file_path,
            "",
            &[wf_lang::WindowSchema {
                name: "test_win".to_string(),
                streams: vec!["events".to_string()],
                time_field: Some("ts".to_string()),
                over: Duration::from_secs(3600),
                fields: vec![
                    wf_lang::FieldDef {
                        name: "ts".to_string(),
                        field_type: wf_lang::FieldType::Base(wf_lang::BaseType::Time),
                    },
                    wf_lang::FieldDef {
                        name: "value".to_string(),
                        field_type: wf_lang::FieldType::Base(wf_lang::BaseType::Digit),
                    },
                ],
            }],
            Arc::clone(&router),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(snapshot_row_count(&router), 2);
    }

    #[tokio::test]
    async fn file_arrow_ipc_replay_routes_rows() {
        let router = make_router("events");
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("events.arrow_ipc");
        let schema = test_schema();
        let batch_a = make_batch(&schema, &[1_000_000_000], &[1]);
        let batch_b = make_batch(&schema, &[2_000_000_000], &[2]);

        {
            let file = std::fs::File::create(&file_path).unwrap();
            let mut writer = FileWriter::try_new(file, &schema).unwrap();
            writer.write(&batch_a).unwrap();
            writer.write(&batch_b).unwrap();
            writer.finish().unwrap();
        }

        replay_arrow_ipc_file(
            &file_path,
            "events",
            &[wf_lang::WindowSchema {
                name: "test_win".to_string(),
                streams: vec!["events".to_string()],
                time_field: Some("ts".to_string()),
                over: Duration::from_secs(3600),
                fields: vec![
                    wf_lang::FieldDef {
                        name: "ts".to_string(),
                        field_type: wf_lang::FieldType::Base(wf_lang::BaseType::Time),
                    },
                    wf_lang::FieldDef {
                        name: "value".to_string(),
                        field_type: wf_lang::FieldType::Base(wf_lang::BaseType::Digit),
                    },
                ],
            }],
            Arc::clone(&router),
            None,
            CancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(snapshot_row_count(&router), 2);
    }
}
