# MoJu Draft Review

基于 `moju/draft/domain/*.facts.json` 从 Rust 代码抽取的事实，做语义合并更新 draft。
`moju verify draft` 全部通过（10+ verify cases）。

---

## 高置信度变更（代码事实，建议直接接受）

### Config Domain

| 变更 | 依据 |
|------|------|
| `FusionConfigLoader.var_context` → `ctx` | `wf-config/src/loader.rs`: 字段名为 `ctx: &'a ConfigVarContext` |
| `WindowConfig` 新增 `evict_policy` 字段 | `wf-config/src/window.rs`: `pub evict_policy: EvictPolicy` |
| 新增 `WindowOverride` struct | `wf-config/src/window.rs`: `pub struct WindowOverride`，用于窗口配置覆盖 |
| 新增 `RawFusionConfigTree` struct | `wf-config/src/loader.rs`: 公开 API，提供 origin tracking |
| 新增 `RawFusionConfigChange` struct | `wf-config/src/loader.rs`: 公开 API，diff 结果 |
| 新增 `ResolvedConfigVar` struct | `wf-config/src/loader.rs`: 公开 API，变量解析结果 |

### Orchestra Domain

| 变更 | 依据 |
|------|------|
| Crate 映射: `wf-runtime` → `wf-engine, wf-runtime` | CLI 入口 (`run_cli`, `Cli`, `Commands`) 在 `wf-engine` crate |
| `Reactor.config, cancel_token` → `cancel, watchers` | `wf-runtime/src/lifecycle/mod.rs`: `cancel: CancellationToken`, `watchers: Vec<JoinHandle<..>>` |
| `BootstrapData` 新增 `schema_count`, `intermediate_targets` | `wf-runtime/src/lifecycle/types.rs`: 包含这 2 个字段 |
| `RunRule.plan` → `executor, window_aliases` | `wf-runtime/src/lifecycle/types.rs`: `executor: RuleExecutor`, `window_aliases: HashMap<..>` |
| `RunRuleKind`: `UserRule, PipelineInternal` → `Match, Each` | `wf-runtime/src/lifecycle/types.rs`: enum 实际变体 |
| `TaskGroup.tasks` → `handles` | `wf-runtime/src/lifecycle/types.rs`: `handles: Vec<JoinHandle<..>>` |
| `ConfigLoadArgs` 字段: `overlay, var` → `overlays, vars` | `moju` 中 `overlay`/`var` 可能是保留字 |
| 新增 `RuntimeReason` state | `wf-runtime/src/error.rs`: `Bootstrap, Shutdown, Core, General` |
| 新增 `EngineEntry` module (layer: Interface) | 对应 `wf-engine` crate 的 CLI 入口 |
| 新增 `CompareConfigLoadArgs` struct | `wf-engine/src/lib.rs`: `--to-config`, `--to-overlay` 等参数 |
| 新增 `ResolvedConfigLoad` struct | `wf-engine/src/lib.rs`: 解析后的 config 加载参数 |

### Engine Domain

| 变更 | 依据 |
|------|------|
| `ProcessEvents` flow 新增 `Ingest`, `Route` 步骤 | `wf-core` 中 batch → route → match → emit 是实际数据流 |

---

## 推断性变更（需要人工审查）

| 变更 | 推断依据 | 风险 |
|------|---------|------|
| `Run` flow 步骤重排: `ResolveConfig → Bootstrap → SpawnTasks → Serve` | 对应 `wf-engine::run_cli()` + `Reactor::start()` 实际流程 | 中 — flow 步骤可能过细 |
| `ReactorStart` flow 新增 5 个 spawn 步骤 | `Reactor::start()` 按序 spawn: alert → evictor → rules → receiver → metrics | 中 — 5 个 step 都创建 `TaskGroup`，最后一个生效 |
| CLI 命令重命名: `RenderConfig` → `RenderConfigCmd`, `DiffConfig` → `DiffCmd` | 避免与 config domain 类型冲突 | 低 — 语义等价 |
| `EngineEntry` 和 `CliEntry` 模块可能存在职责重叠 | 两者都是 Interface 层，都负责 CLI 解析 | 中 — 建议合并为一个模块 |

---

## 模型中有但代码中无（保留，可能为设计意图）

| 项目 | 位置 | 说明 |
|------|------|------|
| `TimerTick` command | orchestra/domain.mju | 定时触发，代码中可能隐式存在 |
| `actor Admin { can RunCli, can RenderConfigCmd, can DiffCmd }` | orchestra/domain.mju | 设计概念，代码中通过 CLI 隐含 |
| `Histogram`, `IntervalRates`, `TotalCounts` | orchestra/domain.mju | 可能在其他 crate 或尚未实现 |

---

## 代码中有但模型可能遗漏

| 类型 | 代码位置 | 建议 |
|------|---------|------|
| `wf-runtime::schema_bridge::schemas_to_window_defs()` | `schema_bridge.rs` | 可能是 `SchemaBridge` module，待确认 |
| `wf-runtime::engine_task::RuleTask` | `engine_task/rule_task.rs` | 运行时 task，非领域概念，可忽略 |
| `wf-runtime::tracing_init::init_tracing()` | `tracing_init.rs` | 基础设施，可忽略 |
| `wf-config::sink::expect::GroupExpectSpec` | `sink/expect.rs` | sink 配置的 expect 功能，可忽略 |

---

## 建议的下一步

1. **人工审查** orchestra domain 的 flow 变更（`Run`, `ReactorStart` 的步骤划分）
2. **合并** `EngineEntry` 和 `CliEntry` 为一个模块
3. **确认** `RunRuleKind.Match/Each` 是否准确捕获了业务含义
4. **确认** draft review 通过后执行: `cp -r moju/draft/domain/* moju/model/domain/`
