# 运行时配置

## 完整配置示例

```toml
mode = "daemon"                              # daemon | batch
sinks = "sinks"

[[sources]]
type = "tcp"
name = "ingress_tcp"
listen = "tcp://127.0.0.1:9800"

[[sources]]
type = "file"
name = "seed_auth"
path = "data/auth_events.ndjson"
stream = "syslog"
format = "ndjson"                            # ndjson | arrow_framed | arrow_ipc

[[sources]]
type = "file"
name = "seed_arrow_framed"
path = "data/auth_events.arrowf"
format = "arrow_framed"                      # wp_arrow 分帧格式

[[sources]]
type = "file"
name = "seed_arrow_ipc"
path = "data/auth_events.arrow"
stream = "syslog"
format = "arrow_ipc"                         # 标准 Arrow IPC file

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"
rules   = "rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.auth_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[window.security_alerts]
mode = "local"
max_window_bytes = "64MB"
over_cap = "1h"

[vars]
FAIL_THRESHOLD = "3"
SCAN_THRESHOLD = "10"
```

## 模式

- `mode = "daemon"`：常驻运行，要求至少一个启用的 `tcp` source
- `mode = "batch"`：批处理回放，要求至少一个启用的 `file` source，且不允许启用 `tcp` source

## Sources

输入统一通过 `[[sources]]` 配置。

### TCP Source

```toml
[[sources]]
type = "tcp"
name = "ingress_tcp"
listen = "tcp://127.0.0.1:9800"
```

说明：

- TCP 接入本身就是 source
- 不再使用 `[server]`
- `daemon` 模式通常通过该入口接收实时数据

### File Source

当前支持三种格式：

| 格式 | 含义 | `stream` |
|------|------|----------|
| `ndjson` | 逐行 JSON | 必填 |
| `arrow_framed` | 当前 `wp_arrow` 分帧文件格式 | 可省略 |
| `arrow_ipc` | 标准 Arrow IPC file | 必填 |

#### `ndjson`

```toml
[[sources]]
type = "file"
path = "data/events.jsonl"
stream = "syslog"
format = "ndjson"
```

#### `arrow_framed`

```toml
[[sources]]
type = "file"
path = "data/events.arrowf"
format = "arrow_framed"
```

说明：

- 文件格式为当前 `wp_arrow` 分帧格式
- 读取方式为 `[4B len][encode_ipc payload]...`
- 默认按帧内 `tag` 路由
- 如有需要，也可显式写 `stream` 覆盖

#### `arrow_ipc`

```toml
[[sources]]
type = "file"
path = "data/events.arrow"
stream = "syslog"
format = "arrow_ipc"
```

说明：

- 标准 Arrow IPC file 不携带业务路由 tag
- 因此必须显式配置 `stream`

### 为什么不用自动识别

`arrow_framed` 与 `arrow_ipc` 都属于 Arrow 相关格式，但语义不同：

- `arrow_framed` 自带逐帧边界和路由 tag
- `arrow_ipc` 是标准文件格式，不包含该路由信息

因此不做自动判别，直接显式写成：

- `arrow_framed`
- `arrow_ipc`
- `ndjson`

## Runtime

```toml
[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"
rules   = "rules/*.wfl"
```

说明：

- `schemas` / `rules` 支持 glob
- 可使用 `schemas/**/*.wfs` 递归扫描

## 窗口默认值与覆盖

全局默认：

```toml
[window_defaults]
max_window_bytes = "256MB"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"
```

按 window 覆盖：

```toml
[window.high_volume_events]
max_window_bytes = "1GB"
over_cap = "1h"
```

## 变量预处理

```toml
[vars]
FAIL_THRESHOLD = "5"
SCAN_THRESHOLD = "10"
```

在 `.wfl` 中引用：

```wfl
fail | count >= $FAIL_THRESHOLD;
```

支持：

- `$VAR`
- `${VAR:default}`

## Sink 路由

告警输出通过 connector-based sink 路由系统配置：

```toml
sinks = "sinks"
```

目录结构：

- `defaults.toml`
- `sink.d/`
- `business.d/`
- `infra.d/`

输出格式取决于 sink：

- raw sink 通常输出扁平 JSON 行
- record sink 可直接输出 Arrow framed / Arrow IPC 等结构化格式

## 输出记录

告警链路在进入 sink 之前，会统一转换成结构化记录：

- 系统字段使用 `__wfu_` 前缀
- `yield (...)` 中的业务字段按原名展开
- 若业务字段与 `__wfu_` 前缀冲突，运行时直接报错

固定系统字段如下：

| 字段 | 说明 |
|------|------|
| `__wfu_id` | 确定性输出 ID |
| `__wfu_rule_name` | 规则名称 |
| `__wfu_score` | 风险评分 |
| `__wfu_entity_type` | 实体类型 |
| `__wfu_entity_id` | 实体标识 |
| `__wfu_origin` | 产出路径：`event` / `close:*` |
| `__wfu_close_reason` | close 原因；event 路径为空字符串 |
| `__wfu_fired_at` | 基于事件时间或 close 水位生成的业务时间 |
| `__wfu_emit_time` | 运行时实际发出该记录的时间 |
| `__wfu_summary` | 引擎生成的摘要 |

示例：

```json
{
  "__wfu_id": "1a2b3c4d",
  "__wfu_rule_name": "brute_force",
  "__wfu_score": 70.0,
  "__wfu_entity_type": "ip",
  "__wfu_entity_id": "10.0.0.1",
  "__wfu_origin": "close:timeout",
  "__wfu_close_reason": "timeout",
  "__wfu_fired_at": "2026-03-11T10:05:00.000Z",
  "__wfu_emit_time": "2026-03-11T10:05:00.123Z",
  "__wfu_summary": "rule=brute_force; scope=[sip=10.0.0.1]; step0=3.0; origin=close:timeout",
  "sip": "10.0.0.1",
  "fail_count": 5
}
```

### `yield_fields` 导出规则

- 标量字段按 schema 类型导出
- 若缺失 schema 类型信息，则按值推断：
  - number -> `float`
  - bool -> `bool`
  - string -> `chars`
- `array/*` 第一版统一退化为 JSON string，再以 `chars` 导出

数组示例：

```json
{
  "ports": "[22,80,443]"
}
```

## 运行引擎

启动：

```bash
wfusion run --config fusion.toml
```

启动流程：

1. 加载并校验 `fusion.toml`
2. 解析 `.wfs`
3. 解析并编译 `.wfl`
4. 创建窗口缓冲区和规则执行器
5. 启动 sources
6. 启动事件调度循环

执行链：

```text
Source -> Router -> WindowStore -> MatchEngine -> YieldWriter -> AlertSink
```

热加载约定：

- 修改 `.wfl` 或 `[vars]` 后可热加载
- 修改 `.wfs` 需要重启
