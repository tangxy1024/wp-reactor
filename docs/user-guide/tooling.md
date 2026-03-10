# 开发与测试工具

## `wfl`

`wfl` 是 WFL 语言的开发者命令行工具，用于规则解释、检查、格式化、离线回放和契约测试。

### 子命令

| 子命令 | 用途 |
|--------|------|
| `explain` | 输出执行计划解释 |
| `lint` | 语义检查和 lint |
| `fmt` | 规则格式化 |
| `replay` | 用 NDJSON 离线回放 |
| `test` | 运行规则契约测试 |
| `verify` | 一步完成 replay + verify |

公共参数：

- `--schemas` / `-s`
- `--var KEY=VALUE`

### `wfl explain`

```bash
wfl explain rules/brute_force.wfl \
    --schemas "schemas/*.wfs" \
    --var FAIL_THRESHOLD=3
```

### `wfl lint`

```bash
wfl lint rules/brute_force.wfl \
    --schemas "schemas/*.wfs" \
    --var FAIL_THRESHOLD=3
```

### `wfl fmt`

```bash
wfl fmt rules/brute_force.wfl
wfl fmt -w rules/*.wfl
wfl fmt --check rules/*.wfl
```

### `wfl replay`

```bash
wfl replay rules/brute_force.wfl \
    --schemas "schemas/*.wfs" \
    --input test_data/events.jsonl \
    --alias fail \
    --var FAIL_THRESHOLD=3
```

限制：

- 离线模式无 window store
- `join` 查找和 `window.has()` guard 返回空值
- EOF 时自动触发 `close_all(Eos)`

### `wfl test`

```bash
wfl test rules/brute_force.wfl --schemas "schemas/*.wfs"
```

### `wfl verify`

推荐写法：

```bash
wfl verify --case brute_force --data-dir data
```

也可手工指定：

```bash
wfl verify rules/brute_force.wfl \
    --schemas "schemas/*.wfs" \
    --input data/brute_force.jsonl \
    --expected data/brute_force.except.jsonl \
    --meta data/brute_force.except.meta.jsonl \
    --format markdown
```

## `wfgen`

`wfgen` 用于生成测试数据，输入是 `.wfg` 场景文件。

### 场景示例

```wfg
use "schemas/security.wfs"
use "rules/brute_force.wfl"

#[duration=30m]
scenario brute_force_detect<seed=42> {
  traffic {
    stream auth_events gen 200/s
  }

  injection {
    hit<30%> auth_events {
      sip seq {
        use(action="failed") with(3,2m)
      }
    }
  }

  expect {
    hit(brute_force_then_scan) >= 95%
  }
}
```

### CLI

生成并发送：

```bash
wfgen gen \
    --scenario examples/count/scenarios/brute_force.wfg \
    --format jsonl \
    --out out/ \
    --send \
    --addr 127.0.0.1:9800
```

一致性校验：

```bash
wfgen lint examples/count/scenarios/brute_force.wfg
```

复用已有数据单独发送：

```bash
wfgen send \
    --scenario examples/count/scenarios/brute_force.wfg \
    --input out/brute_force.jsonl \
    --addr 127.0.0.1:9800
```

对拍验证：

```bash
wfgen verify \
    --actual out/actual_alerts.jsonl \
    --expected out/brute_force.except.jsonl \
    --meta out/brute_force.except.meta.jsonl
```

持续压测：

```bash
wfgen bench \
    --scenario examples/count/scenarios/brute_force.wfg \
    --duration 5m \
    --send \
    --addr 127.0.0.1:9800
```

## 联合验证

推荐直接运行仓库内置 e2e：

```bash
cargo test -p wf-runtime e2e_datagen_brute_force -- --nocapture
```

该用例会自动完成：

1. 生成事件
2. 启动 `wfusion`
3. 通过 TCP 发送 Arrow IPC 数据
4. 将实际告警与 expected 对拍

手工分步调试：

```bash
wfusion run --config examples/wfusion.toml --metrics --metrics-interval 2s
```

```bash
wfgen gen \
    --scenario examples/count/scenarios/brute_force.wfg \
    --format jsonl \
    --out out/ \
    --send \
    --addr 127.0.0.1:9800
```

```bash
wfl verify --case brute_force --data-dir out --format markdown
```
