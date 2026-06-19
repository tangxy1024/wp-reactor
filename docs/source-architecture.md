# Source 架构设计

## 当前架构：connector SourceFactory + WireFormat + BatchSource

warp-fusion 的外部 source（TCP / syslog / Kafka 等）通过 `wp-connector-api` 的
`SourceFactory` 体系构建。connector 层（`wp-core-connectors` 0.5.2+）负责声明并
校验 wire format（`data_format` 参数 → `WireFormat` 枚举）。runtime 通过
`wf-connector-api` 的 `BatchSource` 适配层消费解码后的 Arrow `RecordBatch`。

File source 走 runtime 内联 replay（ndjson / csv / arrow_framed / arrow_ipc）。

```
config.sources
  │
  ├─ kind="file" → 内联 replay (receiver.rs)
  │     ├─ ndjson / csv / arrow_framed / arrow_ipc
  │     └─ replay_* → route_batch
  │
  └─ kind=其他 (tcp/syslog/kafka/…)
        ├─ wp_core_connectors::registry::get_source_factory(kind)
        ├─ factory.validate_spec()                          ← 校验 data_format
        ├─ factory.build(ctx)                               ← wp-connector-api
        │     → SourceSvcIns { acceptor, sources }
        ├─ acceptor.accept_connection(ctrl_rx)              ← 连接接入
        └─ for handle in sources:
              handle.source.start(ctrl_rx)
              WireFormat::from_data_format(data_format)     ← connector 层格式契约
              DataSourceBatchSource::new(source, schema, wire_format)
              loop receive_batch() → Vec<RecordBatch>        ← wf-connector-api
                → route_batch → Router → Window
```

### 三层分工

| 层级 | Crate | 职责 |
|------|-------|------|
| 连接 + 格式契约 | `wp-connector-api` / `wp-core-connectors` | `SourceFactory` 构建 + `validate_spec` 校验 `data_format` |
| Wire format 定义 + 解码 | `wp-core-connectors` (`sources/batch/arrow.rs`) | `WireFormat` 枚举 + `decode_arrow_ipc_batches` / `decode_arrow_framed_batches` |
| Arrow 消费 | `wf-connector-api` (`BatchSource`) | trait 定义；runtime 通过 `DataSourceBatchSource` 实现 |

### WireFormat（connector 层的格式契约）

`wp-core-connectors` 0.5.2 在 `sources/batch/arrow.rs` 定义：

```rust
pub enum WireFormat {
    Ndjson,       // JSON Lines 文本
    ArrowStream,  // 原始 Arrow IPC Stream (schema + batch + EOS)
    ArrowFramed,  // wp_arrow 帧: [4B tag_len][tag][Arrow IPC Stream]
}
```

从 source spec 的 `data_format` 参数解析：

```rust
WireFormat::from_data_format(source.params.get("data_format").map(|s| s.as_str()))
```

TCP / file source factory 的 `validate_spec` 会校验 `data_format` 值是否合法，
在启动阶段就暴露配置错误。

### DataSourceBatchSource（runtime 适配层）

`DataSourceBatchSource`（`wf-runtime/src/source/mod.rs`）桥接 `DataSource` →
`BatchSource`：

- 包装 `Box<dyn DataSource>`
- 按 `WireFormat` 分派解码（NDJSON / ArrowStream / ArrowFramed）
- **ArrowFramed 额外提取 tag**：通过 `wp_arrow::decode_ipc` 解码，保留帧头中的
  stream 名（tag），供 runtime 在未配置 `stream` 参数时用作路由 stream 名
- EOF 正确映射：`wp_connector_api::SourceReason::EOF` → `wf_connector_api::SourceReason::EOF`

ArrowStream / Ndjson 的解码逻辑直接委托 connector 层的共享函数
（`decode_arrow_ipc_batches` / `ndjson_to_record_batch`），不重复实现。

### 为什么 `BatchSource` trait 不定死 wire format

`BatchSource::receive_batch()` 返回 `Vec<RecordBatch>`（已解码），不关心 payload
原本是什么格式。这允许第三方直接 impl `BatchSource`，按自己的方式构造
`RecordBatch`，不需要经过 `DataSource` 或 `WireFormat`。格式契约只在 connector
实现层（`wp-core-connectors`），trait 层保持格式无关。

---

## `wf-connector-api` BatchSource trait

```rust
#[async_trait]
pub trait BatchSource: Send {
    async fn start(&mut self) -> SourceResult<()> { Ok(()) }
    async fn receive_batch(&mut self) -> SourceResult<Vec<RecordBatch>>;
    async fn close(&mut self) -> SourceResult<()> { Ok(()) }
    fn identifier(&self) -> &str;
}
```

runtime 中的消费者是 `DataSourceBatchSource`（`wf-runtime/src/source/mod.rs`）。

### 历史背景

早期 warp-fusion 的 Source 层全部内联管理（`SourceConfig` enum + `Receiver` +
`replay_*`），没有复用 `wp-connector-api`。随着 `arrow-tcp-stream-compatibility.md`
设计文档的实施，外部 source 已迁移到 `SourceFactory` 体系。`Receiver` struct 及
其内联 TCP handler 已被删除。

`wp-core-connectors` 0.5.0 的 `TcpBatchSource` / `FileBatchSource` 只支持 NDJSON。
0.5.2 新增了 `WireFormat` + Arrow 解码，实现了完整的格式契约。
