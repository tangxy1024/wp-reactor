# Provider Window 工作计划

## Phase 1：ProviderWindow 基础类型

**目标**：ProviderWindow 实现 WindowLookup trait，能从外部数据源返回数据。

**任务**：
- [ ] 定义 `ProviderWindow` 结构体（table, query, refresh, cache: HashMap）
- [ ] 实现 `WindowLookup` trait（snapshot, snapshot_with_timestamps, snapshot_field_values）
- [ ] 注册到 `WindowRegistry`，与 `BufferWindow` 共存
- [ ] 单元测试：Mock Provider，验证 lookup 行为

**产出**：`wf-engine/src/window/provider.rs`

---

## Phase 2：knowdb facade + 全量加载

**目标**：ProviderWindow 通过 wp-knowledge facade 加载数据到本地 HashMap。join 走内存查找，零 IO。

**任务**：
- [ ] `KnowdbProvider::load_all()`：bootstrap 时 `facade::query(query)` → 全量加载到 HashMap
- [ ] `WindowLookup::snapshot()` → 遍历 HashMap，返回 Vec<HashMap<String, Value>>
- [ ] `refresh` 定时器：到期清空 HashMap，重新 `load_all()`
- [ ] 集成测试：knowdb.toml + CSV → ProviderWindow → join 匹配正确

**设计决策**：全量加载 + HashMap 是默认策略。join 操作全部走内存，性能最优。一事件一 SQL 是反模式。

**产出**：`wf-engine/src/window/provider/knowdb.rs`

---

## Phase 3：wfusion.toml 配置支持

**目标**：通过 `wfusion.toml` 声明 Provider 窗口。

**任务**：
- [ ] `[window.X]` 增加 `table`、`query`、`refresh` 字段
- [ ] bootstrap 时解析 knowdb.toml，为每个 `[window.X]` 创建 ProviderWindow
- [ ] 默认行为：无 `query` → `SELECT * FROM <table>`，无 `refresh` → 静态不刷新
- [ ] 验证：`table` 必须指向 knowdb.toml 中存在的表

**产出**：`wf-config` 配置扩展 + `wf-runtime` bootstrap 逻辑

---

## Phase 4：迁移当前实现

**目标**：删除 `load_knowledge_into_windows`，全部走 ProviderWindow。

**任务**：
- [ ] `port_scan_whitelist` 示例改用 ProviderWindow
- [ ] 删除 `bootstrap.rs` 中 CSV 直接加载逻辑
- [ ] 清理 `build_record_batch_from_json` pub(crate) 暴露
- [ ] 回归测试：所有示例 + wfl test 通过

**产出**：代码简化，统一入口

---

## Phase 5（可选）：大表按需加载

**场景**：百万级表（>1M 行），全量加载内存不够。

**任务**：
- [ ] `load_strategy = "full" | "on_demand"`，默认 full
- [ ] on_demand：join 时 `facade::query_row("SELECT ... WHERE key = ?", [val])` + 本地 LRU
- [ ] 阈值建议：< 100K 行用 full，> 100K 用 on_demand

**注意**：这是特殊场景。大部分场景（白名单、配置表）full-load 是最优解。

---

## 依赖关系

```
Phase 1 ──→ Phase 2 ──→ Phase 3 ──→ Phase 4 (迁移)
                              │
                              └──→ Phase 5 (可选)
```
