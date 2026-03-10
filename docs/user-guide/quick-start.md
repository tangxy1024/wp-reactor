# 快速开始

## 概述

WFL 是 WarpFusion 的检测领域专用语言（DSL），用于编写安全关联检测规则、风险告警归并与实体行为分析逻辑。

核心设计理念：

- 简洁可读
- 显式优先
- 可解释可调试

WFL 不是通用流计算 SQL，也不是任意 DAG 引擎。

## 第一个项目

一个典型的 WarpFusion 项目包含三类文件：

```text
my-project/
├── fusion.toml
├── schemas/
│   └── security.wfs
├── rules/
│   └── brute_force.wfl
└── sinks/
```

## 第一个规则

第 1 步，定义数据窗口 `schemas/security.wfs`：

```wfs
window auth_events {
    stream = "syslog"
    time = event_time
    over = 5m

    fields {
        sip: ip
        username: chars
        action: chars
        event_time: time
    }
}

window security_alerts {
    over = 0
    fields {
        sip: ip
        fail_count: digit
        message: chars
    }
}
```

第 2 步，编写规则 `rules/brute_force.wfl`：

```wfl
use "security.wfs"

rule brute_force {
    events {
        fail : auth_events && action == "failed"
    }

    match<sip:5m> {
        on event {
            fail | count >= 3;
        }
    } -> score(70.0)

    entity(ip, fail.sip)

    yield security_alerts (
        sip = fail.sip,
        fail_count = count(fail),
        message = fmt("{} brute force detected", fail.sip)
    )
}
```

第 3 步，配置运行时 `fusion.toml`：

```toml
mode = "daemon"
sinks = "sinks"

[[sources]]
type = "tcp"
name = "ingress_tcp"
listen = "tcp://127.0.0.1:9800"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"
rules   = "rules/*.wfl"

[window_defaults]
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[vars]
FAIL_THRESHOLD = "3"
```

第 4 步，启动引擎：

```bash
wfusion run --config fusion.toml
```

如需直接看到指标快照：

```bash
wfusion run --config fusion.toml --metrics --metrics-interval 2s
```

## 三文件模型

WFL 采用职责分离的三文件模型：

| 文件 | 扩展名 | 职责 | 热加载 |
|------|--------|------|:------:|
| Window Schema | `.wfs` | 逻辑数据定义（window、field、time、over） | 否 |
| 检测规则 | `.wfl` | 检测逻辑（bind/match/join/yield） | 是 |
| 运行时配置 | `.toml` | 物理参数（mode、sources、watermark、sinks） | 仅 `[vars]` |

依赖关系如下：

```text
.wfs
  ↑
.wfl
  ↑
.toml
```

- `.wfl` 仅能引用 `use` 导入的 window
- `.toml` 只管物理参数，不写业务规则
- `.wfs` 变更需要重启引擎

## 模式说明

- `mode = "daemon"`：常驻运行，至少需要一个启用的 `tcp` source
- `mode = "batch"`：批处理回放，至少需要一个启用的 `file` source，且不允许启用 `tcp` source

## Source 约定

`wfusion` 的输入统一通过 `[[sources]]` 声明：

- `type = "tcp"`：基于 TCP 接收数据
- `type = "file"`：基于文件回放数据

不再使用 `[server]` 配置块。

更多配置见 [运行时配置](./runtime-config.md)。
