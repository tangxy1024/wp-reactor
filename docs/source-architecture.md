# Source 架构设计

## 为什么不用 wp-connector-api 的 Source 抽象

warp-fusion 的 Source 层是自己管理的（`SourceConfig` enum + `Receiver` + `replay_*`），没有复用 `wp-connector-api` 的 `SourceFactory` / `DataSource` trait 体系。这是有意为之。

### 两种 Source 模型

| | wp-connector-api Source | warp-fusion Source |
|---|---|---|
| 面向场景 | 通用数据摄取（为下游 parse 管线准备原始数据） | 专用 CEP 引擎（直接产出行列存 batch，进窗口） |
| 数据粒度 | 逐条 `SourceEvent` | 批量 `RecordBatch`（数千行） |
| 数据类型 | `RawData`（String / Bytes / ArcBytes） | Arrow `RecordBatch` |
| 路由方式 | `src_key: SmolStr` 标签 | `stream_name` → `Router::route()` |
| 消费模型 | `async fn receive()` 返回 `Vec<SourceEvent>` | `router.route(stream, batch)` 同步批量路由 |
| 元数据 | event_id、tags、ups_ip、preproc | 无（纯数据列） |

### 为什么不兼容

**1. 数据类型不同**

warp-fusion 全链路是面向 Arrow 零拷贝设计的：

```
RecordBatch → router.route() → window.append_with_watermark() → state machine.advance()
```

每一步都在操作 Arrow 列存。如果从 `SourceEvent { payload: RawData }` 接入，每条消息都需要：

```
RawData → 解析 NDJSON/Arrow → 重新组装 RecordBatch → 喂给 router
```

即使 `RawData` 能直接装 Arrow 二进制，`receive()` 仍然返回 `Vec<SourceEvent>`——多条消息需要手动拼成一个 `RecordBatch`，这个"拼"的逻辑省不掉。

**2. 处理模式不同**

wp-connector-api 的 `DataSource` 是通用抽象：`receive()` 拿到一批原始事件，下游 parse 管线自行解析。这是"拿原数据，自己看着办"。

warp-fusion 需要的是"拿结构化数据，直接进窗口"。`replay_ndjson_file` 把解析和路由合在一起，省去中间序列化/反序列化：

```rust
// 当前的实现：解析 + 路由一步完成
fn replay_ndjson_file(...) {
    for line in file {
        let json = serde_json::from_str(&line)?;
        let batch = json_to_record_batch(json, schema)?;
        router.route(stream_name, batch)?;  // 直接进窗口
    }
}
```

**3. 依赖膨胀**

引入 `SourceFactory` 体系会带进来 `wp_model_core::RawData`、`SourceBuildCtx`、`Tags`、`ControlEvent` 等一整套抽象，而 warp-fusion 只需要一个简单的"从某处读数据 → 解析 → 路由"循环。

### 什么时候应该用 wp-connector-api Source

以下场景适合：

- 下游有独立的 parse 管线，Source 只需要产出原始数据
- 需要统一的 source 配置格式和 registry 管理
- 多个项目共享同一套 source connector 实现
- 数据源种类很多（10+ 种），需要工厂模式统一管理

### warp-fusion 的选择

当前 warp-fusion 的 Source 只有 3 种（TCP、File、Kafka），且每种的数据解析逻辑和路由紧密耦合。用 enum + 独立 replay 函数比工厂模式更简单：

```rust
// 当前方式：直观，无抽象成本
pub enum SourceConfig { Tcp(TcpSourceConfig), File(FileSourceConfig), Kafka(KafkaSourceConfig) }

match source {
    SourceConfig::Tcp(tcp) => { Receiver::bind(...).run().await }
    SourceConfig::File(file) => { replay_ndjson_file(...).await }
    SourceConfig::Kafka(k) => { replay_kafka(...).await }
}
```

如果未来 source 种类增长到需要动态注册（如 plugin 体系），再迁移到 `SourceFactory` 抽象。
