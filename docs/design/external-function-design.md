# External Function 设计

> WFL 调用外部服务的函数机制
> 状态：Draft for review | 创建：2026-06-12

## 1. 背景与动机

### 1.1 问题

WFL 规则中需要查询大规模外部数据（威胁情报、弱口令库、IP 归属地等），但当前机制存在瓶颈：

| 当前能力 | 工作原理 | 规模上限 |
|---------|---------|:--------:|
| `join snapshot` | 全量加载到 `Vec<HashMap>`，线性扫描 | < 10 万 |
| `window.has()` | 同上，全量加载 + 遍历 | < 10 万 |
| `password in (...)` | 编译期内联列表 | < 100（规则体积约束） |

现实中的外部数据远超此量级——威胁情报 IP 库 10 亿+、Have I Been Pwned 密码库 10 亿+、企业内部泄露凭据百万级。这些数据**不可能全量加载到 WFL 进程内**，需要"逐条点查询"的能力。

### 1.2 目标

提供一个 `external()` 函数，允许 WFL 规则在运行时调用外部服务做点查询，而不将数据加载到 window 内存中。

### 1.3 非目标

- 不做流式/批量查询（第一阶段逐条调用）
- 不改造现有 join 语法（`join via external(...)` 为第二阶段）
- 不定义外部服务的具体实现（HTTP/gRPC/Redis 由 connector 承载）

---

## 2. 函数模型

### 2.1 语法

```wfl
external("<service_name>", <arg1>, <arg2>, ...)
```

- `service_name`：字符串字面量，对应 `wfusion.toml` 中 `[external.<name>]` 配置的服务名
- `args`：至少一个参数，类型任意（运行时传值，不做编译期类型约束）
- 返回值：`bool`（典型场景），也可以扩展为 `float`/`chars`

### 2.2 调用位置

| 位置 | 是否允许 | 说明 |
|------|:------:|------|
| `events` event filter | 待定 | 编译后可支持，但可能延迟事件绑定 |
| `on event` match step guard | ✅ 允许 | 主要使用位置 |
| `and close` / `on close` guard | ✅ 允许 | 关闭阶段同样可用 |
| `on each` where 表达式 | ✅ 允许 | 逐条求值场景 |
| `yield` 参数表达式 | 待定 | rich response 需要 `external()` 返回结构体，第一阶段不支持 |
| `score()` 表达式 | ✅ 允许 | 如 `score(if external(...) then 90.0 else 10.0)` |

### 2.3 返回值

| 返回类型 | 典型场景 | 示例 |
|---------|---------|------|
| `bool` | 判定式查询 | `external("password_check", e.password_hash)` |
| `float` | 置信度查询 | `external("threat_intel", e.dip, "confidence")` |
| `chars` | 标签查询 | `external("geoip", e.sip, "country")` |

第一阶段（P0）优先实现 `bool` 和 `float`。`chars` 在 P1 实现。

### 2.4 语义

- `external()` 是**同步阻塞**调用（对规则执行而言），每次调用等待外部服务返回
- 调用失败/超时时，按配置的 `on_error` 策略返回默认值
- 返回值可参与所有表达式运算（比较、逻辑、算术、`if/then/else`）

---

## 3. 典型用例

### 3.1 弱口令检测（布尔判定）

```wfl
rule weak_password_login {
    events {
        e : auth_events && e.service == "ssh" && e.result == "success"
    }
    on each e where external("password_check", e.password_hash) -> score(75.0)
    entity(ip, e.sip)
    yield security_alerts (
        sip = e.sip,
        user = e.user,
        alert_type = "weak_password",
        detail = fmt("user '{}' used known weak password", e.user)
    )
}
```

与 `join snapshot` + `isnotnull` 方案相比：
- 不需要 `weak_password_db` window
- 不需要加载密码库到内存
- 不需要 `isnotnull` guard
- 规模由外部服务承担（10 亿级无压力）

### 3.2 威胁情报 IP 检查（浮点数置信度）

```wfl
rule malicious_ip_connection {
    events {
        c : conn_events && is_outbound(c.dip)
    }
    on each c where external("threat_intel", c.dip, "confidence") > 0.8 -> score(90.0)

    // 或者结合窗口聚合
    match<sip:5m> {
        on event {
            c && external("threat_intel", c.dip, "confidence") > 0.8 | count >= 3;
        }
    } -> score(85.0)

    entity(ip, c.sip)
    yield security_alerts (
        sip = c.sip,
        dip = c.dip,
        alert_type = "threat_intel_hit",
        detail = fmt("multiple connections to malicious IP {}", c.dip)
    )
}
```

### 3.3 GEO IP 归属（字符串标签，P1）

```wfl
rule geo_anomaly {
    events {
        c : conn_events
    }
    on each c where external("geoip", c.dip, "country") == "KP" -> score(60.0)
    entity(ip, c.sip)
    yield security_alerts (
        sip = c.sip,
        dip = c.dip,
        country = external("geoip", c.dip, "country"),
        alert_type = "geo_anomaly",
        detail = fmt("connection to {}", external("geoip", c.dip, "country"))
    )
}
```

> 注：`yield` 中调用 `external()` 意味着同一事件对外部服务有多次调用（where + yield × N）。P1 需要优化方案（详见 §7）。

---

## 4. 配置模型

### 4.2 配置模型

P0 阶段仅支持 Redis 后端。HTTP/gRPC 后端在 P1 引入。

wfusion 作为 Redis client，Redis 服务端承载 Bloom filter / Hash / Set 等数据结构。

```toml
# Bloom filter — 大规模存在性判定（10 亿级）
[external.password_check]
type = "redis"
endpoint = "redis://127.0.0.1:6379"
command = "BF.EXISTS"               # RedisBloom 命令
key = "weak_passwords"              # Bloom filter 名称
timeout = "10ms"
cache = "10000"
on_error = "false"

# Hash — 等值查表（百万级，可返回标签）
[external.known_actor]
type = "redis"
endpoint = "redis://127.0.0.1:6379"
command = "HGET"
key = "known_actors"
timeout = "10ms"
cache = "10000"
on_error = "false"
```

调用映射：

```
external("password_check", e.password_hash)
  →  BF.EXISTS weak_passwords <e.password_hash>
  →  返回 1/0 → bool

external("known_actor", e.dip)
  →  HGET known_actors <e.dip>
  →  返回 "APT29" / nil → chars / null
```

### 4.3 字段说明

| 字段 | 类型 | 默认值 | 说明 |
|------|------|:-----:|------|
| `type` | string | **必填** | P0 固定为 `"redis"` |
| `endpoint` | string | **必填** | Redis URL（`redis://127.0.0.1:6379`） |
| `command` | string | **必填** | Redis 命令：`BF.EXISTS` / `GET` / `HGET` / `SISMEMBER` |
| `key` | string | **必填** | Redis key 名：Bloom filter 名、Hash 名、Set 名 |
| `timeout` | duration | `"100ms"` | 单次调用超时 |
| `cache` | int | `0` | Client-side LRU 缓存容量 |
| `on_error` | string | `"false"` | 超时/错误时的返回值 |

调用映射：

```
external("password_check", e.password_hash)
  →  BF.EXISTS weak_passwords <e.password_hash>  →  1/0 → bool

external("known_actor", e.dip)
  →  HGET known_actors <e.dip>  →  "APT29" / nil → chars
```

### 4.4 两层架构：Client Cache + Redis

### 4.4 两层架构：Client Cache + Redis

`external()` 采用 Client Cache + Redis 两层架构。

```
┌─ wfusion 进程内 ───────────────────────┐
│                                         │
│  RuleTask                               │
│    │                                    │
│    ├─ ① Client Cache (LRU, 进程内)      │
│    │    hit → 返回 (< 0.01ms)            │
│    │    miss → ↓                        │
│    │                                    │
│    ├─ ② Redis Client (连接池)            │
│    │    BF.EXISTS weak_passwords <hash>  │
│    │    成功 → 写入 cache + 返回 (~0.1ms) │
│    │    超时/错误 → on_error 兜底         │
│    │                                    │
└────┼────────────────────────────────────┘
     │  localhost
┌────▼────────────────────────────────────┐
│  Redis Server + RedisBloom module       │
│  - Bloom filter 10 亿级 O(k)            │
│  - Hash/Set 原生支持 O(1)                │
│  - 多 wfusion 实例共享                   │
└─────────────────────────────────────────┘
```

| 层 | 解决的问题 | 局限 |
|----|-----------|------|
| Client Cache（wfusion 进程内） | 消除重复查询的 IPC | 内存容量受限，只能缓存热点 |
| Redis（独立进程） | 10 亿级全量数据，O(k)/O(1)，多实例共享 | localhost IPC ~0.1ms |

**缓存命中率决定吞吐上限：**

| 命中率 | 有效延迟 | 单核 QPS |
|:------:|---------|:--------:|
| 90% | ~0.02ms | ~50K/s |
| 99% | ~0.011ms | ~90K/s |
| 99.9% | ~0.01ms | ~100K/s |

密码库场景天然高命中率——已知弱口令就几千个，大部分登录事件的密码哈希集中在少数热门弱口令上。威胁情报 IP 同理，恶意 IP 数量远小于总 IP 空间。

**Client Cache 容量建议：**

| 场景 | 热点数量 | 建议 cache 容量 |
|------|:------:|:-------------:|
| 弱口令库 | 数千 | 10,000 |
| 威胁情报 IP | 数万 | 50,000 |
| GEO IP | IP 基数大，命中分散 | 100,000 |

### 4.5 Client Cache 机制

```
cache_key = hash(service_name + "\x00" + arg1 + "\x00" + arg2 + ...)
```

- `arg` 使用 WFL `Value::Str` / `Value::Number` 序列化后的字符串形式
- 相同的 `(service, args...)` 复用缓存结果
- 缓存不设 TTL（密码/威胁情报数据相对稳定），依赖 LRU 容量驱逐
- 缓存命中时不计入 `timeout`，不产生网络调用
- 当 Redis 不可用时，已有缓存条目继续可用（降级不降死）

### 4.6 Redis + RedisBloom 服务端

wfusion 不做查表逻辑，Redis 承载全部数据结构和查询。

**Bloom filter（大规模存在性判定）：**

```bash
# 服务端部署 Redis + RedisBloom module
redis-server --loadmodule ./redisbloom.so

# 离线预生成 + 加载 Bloom filter
redis-cli BF.RESERVE weak_passwords 0.0001 1000000000
# 加载 10 亿条弱口令哈希...
redis-cli BF.MADD weak_passwords e10adc... 5f4dcc... ...
```

wfusion 侧只需 Redis Client 发 `BF.EXISTS`，不感知数据量。

**HashMap（中等规模标签查询）：**

```
HSET known_actors 10.0.0.1 "APT29"
HGET known_actors 10.0.0.1  →  "APT29"
```

**Redis 支持的数据结构：**

| Redis 数据结构 | 对应命令 | 查询复杂度 | 适用场景 |
|--------------|---------|:--------:|---------|
| Bloom filter | `BF.EXISTS key value` | O(k) | 弱口令、恶意 IP 存在性判定（10 亿级） |
| Hash | `HGET key field` | O(1) | IP → actor、域名 → category（百万级） |
| Set | `SISMEMBER key member` | O(1) | 白名单排除（十万级） |
| String | `GET key` | O(1) | 简单 KV 查表 |

### 4.7 性能画像与方案选择

| 规模 | 预期 QPS | 延迟要求 | 推荐方案 |
|------|:------:|:--:|------|
| < 10 万 | < 1K/s | < 50ms | `join snapshot` 进程内（不需要 external） |
| 10 万 ~ 1000 万 | < 10K/s | < 1ms | `external()` + Redis（低 QPS 时 IPC 可接受） |
| 10 万 ~ 1000 万 | > 10K/s | < 1ms | `external()` + 大容量 Client Cache（高命中率掩盖 IPC） |
| > 1000 万 | 任意 | < 1ms | `external()` + Redis + RedisBloom（独立进程承载全量） |

核心判断：**不是"内嵌 vs 独立"的立场问题，是"跑多少量"的工程选择。**

### 4.8 与 Connector 模型的关系

```
sink connector:  kind = "file" | "tcp" | "kafka" | ...
external:        type = "redis" | "http" | "grpc"
```

Redis external connector 与 sink 的 `kind` 分开管理。`external` 是独立的 connector direction——不做数据写出，只做请求-响应式点查询。实现层复用 `wp-connector-api` 的工厂模式。

---

## 5. 调用模型

### 5.1 整体流程

```
RuleTask main loop
  │
  ├─ pull_and_advance()
  │    for event in events:
  │      eval guard: e && external("password_check", e.password_hash)
  │        │
  │        ├─ ① 查 Client Cache
  │        │    key = hash("password_check" + "\x00" + e.password_hash)
  │        │    hit? → 返回缓存值，跳到步骤③
  │        │
  │        ├─ ② Redis Client 发送命令
  │        │    BF.EXISTS weak_passwords <e.password_hash>
  │        │    1  → 缓存 true，返回 true
  │        │    0  → 缓存 false，返回 false
  │        │    超时/错误 → 按 on_error 返回
  │        │
  │        └─ ③ 继续 guard 求值
  │             e && true → Advance → Matched → alert
  │             e && false → Accumulate
```

### 5.2 并发语义

- 每条 RuleTask 内串行调用 `external()`（同步模式）
- 不同 RuleTask 之间并行，共享同一个 `ExternalRuntime` 实例（内部连接池）
- `external()` 调用期间**不释放** RuleTask 对 `CepStateMachine` 的独占权
- 如果 `external()` 调用时间过长，会阻塞该规则的后续事件处理

### 5.3 延迟预算

```
单次事件处理时间 = guard 求值 + external() 调用 + 状态机推进 + alert 生成

目标: P99 < 5ms @ 10K EPS (不含 external)
external() 预算:
  - cache hit:  < 0.1ms (内存查表)
  - cache miss: < timeout (典型 50-100ms)
```

当 `external()` 在 hot path（`on event` guard）中使用时，cache 命中率至关重要。

---

## 6. 错误策略

### 6.1 on_error 的四种模式

| 模式 | 行为 | 适用场景 |
|------|------|---------|
| `"false"` | 返回布尔 `false` | 判定式查询（安全兜底：宁可漏报不可误报） |
| `"true"` | 返回布尔 `true` | 白名单查询（安全兜底：宁可误报不可漏报） |
| `"0.0"` | 返回浮点数 `0.0` | 置信度查询（错误时置信度为 0） |
| `"ignore"` | 跳过当前事件，不推进状态机 | 关键查询（不确定时不做判定） |

### 6.2 指标暴露

以下指标由 `wf-runtime` 的 metrics 子系统暴露：

| 指标 | 类型 | 说明 |
|------|------|------|
| `wf_external_call_total{service, status}` | Counter | `status = success \| timeout \| error \| cache_hit` |
| `wf_external_latency_seconds{service}` | Histogram | 外部调用耗时分布 |
| `wf_external_cache_hit_ratio{service}` | Gauge | 缓存命中率 |
| `wf_external_error_fallback_total{service, on_error}` | Counter | fallback 触发次数 |

### 6.3 运维告警建议

| 条件 | 建议动作 |
|------|---------|
| `cache_hit_ratio < 0.5` 持续 5 分钟 | 增大 cache 容量或检查 key 分布 |
| `timeout` 占比 > 0.01 持续 5 分钟 | 增大 timeout 或检查外部服务健康 |
| `error` 占比 > 0.001 | 检查外部服务可用性 |
| `on_error_fallback` 速率 > 100/min | 检查网络/服务 | + 确认 on_error 策略合适 |

---

## 7. 性能约束与后续优化（P1）

### 7.1 当前约束

- **逐条同步调用**：每个事件一次网络往返（cache miss 时），延迟预算紧张
- **不可在 yield 中多次调用**：`yield` 中 `external("geoip", e.dip, "country")` 如果同一事件出现多次，会造成重复网络调用
- **不可做批量查询**：1000 个事件 = 1000 次 HTTP 请求

### 7.2 P1 优化方向

**调用结果缓存到 EvalCtx**

同一事件在 guard + yield 多处引用时，仅调用一次 `external()`，结果缓存到 eval context：

```
on each c where external("geoip", c.dip, "country") == "KP" -> score(60.0)
yield alerts (
    country = external("geoip", c.dip, "country"),  // 复用 where 的结果
)
```

**批量异步调用**

```wfl
// P1 语法: 批量查询，一次性把窗口内所有待求值事件发给外部服务
match<sip:5m> {
    on event batch {
        c && external_batch("threat_intel", c.dip) > 0.8 | count >= 3;
    }
}
```

runtime 在 match step 求值前，收集当前窗口内所有待求值事件的 `c.dip`，一次 HTTP batch 请求拿到所有结果，再分别判定。

**Rich response 与 enrich 语法**

```wfl
// P1 语法: external 返回结构体，用于 join 式富化
join via external("threat_lookup", e.dip) -> (threat_type, actor, confidence)
```

这一阶段暂时不展开详细设计。

---

## 8. 安全边界

### 8.1 网络

| 约束 | 说明 |
|------|------|
| 默认仅允许 loopback | `endpoint` 默认要求 `127.0.0.1` / `::1`。非 loopback 需显式配置 `allow_external_network = true` |
| 内网限定 | 支持 CIDR 白名单 `allowed_networks = ["10.0.0.0/8", "172.16.0.0/12"]` |
| 公网禁止 | 生产环境禁止公网调用（`allow_public_network = false`，默认） |

### 8.2 TLS

- `tls = true` 时使用系统根证书
- `tls_ca` 可指定自定义 CA
- mTLS 在 P1 支持（`tls_cert` + `tls_key`）

### 8.3 认证

| 认证方式 | 配置字段 | 优先级 |
|---------|---------|:-----:|
| 无认证 | — | P0 |
| API Key Header | `api_key_header = "X-API-Key"`, `api_key` | P0 |
| Bearer Token | `bearer_token` 或 `bearer_token_file` | P1 |
| mTLS | `tls_cert`, `tls_key` | P1 |

### 8.4 审计

每次 `external()` 调用记录 audit log（DEBUG 级别）：
- 时间戳
- service name
- 输入参数（脱敏：密码类服务只记录 hash 前缀）
- 耗时
- 结果（命中/未命中/错误）

---

## 9. 编译器支持

### 9.1 语法层

在 `wf-lang` AST 中新增表达式变体：

```rust
pub enum Expr {
    // ... existing variants ...
    ExternalCall {
        service: String,         // 编译期字符串字面量
        args: Vec<Expr>,         // 运行时求值的参数列表
        expected_return: ValType, // Bool | Float | Chars
    },
}
```

### 9.2 语义约束

| ID | 规则 |
|----|------|
| EXT1 | `service` 必须是 STRING 字面量，不能是变量/表达式 |
| EXT2 | `args` 至少一个 |
| EXT3 | `external()` 在 per-event 上下文中使用（`on event` guard / `on each` where）。不允许在 `events` 的窗口级过滤中使用（P0 阶段暂不支持） |
| EXT4 | `external()` 返回值类型由调用上下文推断（`> 0.8` → Float，`== "KP"` → Chars，独立使用 → Bool） |
| EXT5 | 编译器不校验 `service` 是否在配置中已定义（运行时抛错） |
| EXT6 | 返回类型与比较操作符的类型规则跟随现有类型系统（T7-T10） |

### 9.3 编译产物

`ExternalCall` 编译为 `ExprPlan::ExternalCall`，包含：
- `service: String`：服务名
- `arg_plans: Vec<ExprPlan>`：参数求值计划
- `return_type: ValType`：预期返回类型

---

## 10. 实现 Plan

### Phase 0（最小可用）

**wf-lang / wf-engine 侧**：
- [ ] `ExternalCall` AST + parser + checker（`bool` 返回值）
- [ ] `ExternalRuntime` trait + Redis connector（`redis` crate + 连接池）
- [ ] Client-side LRU cache
- [ ] `wf-runtime` 指标埋点
- [ ] `wfusion.toml` `[external]` 配置解析

**Redis 服务端**（运维部署，非 wfusion 代码）：
- [ ] Redis Server + RedisBloom module 安装
- [ ] Bloom filter 离线预生成脚本（`md5(password_list.txt)` + `BF.MADD`）
- [ ] 数据导入 + 验证

### Phase 1

- [ ] `chars` 返回值支持
- [ ] 同事件内调用结果复用（EvalCtx 缓存）
- [ ] 批量调用语义（`external_batch`）
- [ ] 富化语法（`join via external(...) -> (fields)`）
- [ ] HTTP / gRPC connector

### Phase 2

- [ ] 异步调用 + 流水线化
- [ ] 缓存 TTL + 主动刷新
- [ ] Redis Cluster / Sentinel 支持

---

## 11. 开放问题

1. **`external()` 是否允许在 event filter 中使用？**
   ```
   events {
       e : auth_events && external("check", e.password_hash)
   }
   ```
   优点：与现有 filter 语法一致。缺点：filter 在事件绑定时执行，此时 `e.password_hash` 尚不可用（事件尚未解析）。建议 P0 不允许，P1 评估。

2. **`on_error = "ignore"` 时，如何处理窗口状态机？**
   当前状态机已经到了 `advance(event)`，`ignore` 意味着该事件被静默丢弃。如果该事件刚好是 match step 的关键事件（如第 3 次失败），状态会缺失。策略：
   - `ignore` 仅适用于 `on each`（无状态）
   - 或 `ignore` 在 `match` 中降级为 `false`（不推进但不跳过事件）
   - 建议：P0 不支持 `"ignore"`，仅支持 `false`/`true`/`0.0`

3. **缓存一致性：外部数据更新后，缓存何时失效？**
   P0 无 TTL（靠 LRU 容量驱逐）。需要主动失效的场景（如威胁情报更新）需外部系统通知 `wfusion` 清缓存。P1 再议。

---

## 12. 相关文档

- WFL v2.1 设计方案 → [wfl-desion.md](wfl-desion.md)
- WarpFusion 设计方案 → [warp-fusion.md](warp-fusion.md)
- weak_password 示例 → `warp-fusion/examples/weak_password/README.md`
