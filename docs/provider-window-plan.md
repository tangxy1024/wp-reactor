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

## Phase 2：knowdb facade 集成

**目标**：ProviderWindow 通过 wp-knowledge facade 查询真实数据。

**任务**：
- [ ] `KnowdbProvider`：封装 `facade::query()`，作为 ProviderWindow 的后端
- [ ] 支持 CSV 路径（当前 knowdb 能力）→ SQLite 自动构建 → facade query
- [ ] 预留 Postgres/MySQL 扩展点（facade 已支持，只需 knowdb.toml 配置）
- [ ] 集成测试：knowdb.toml + CSV → ProviderWindow → 查询返回正确行

**产出**：`wf-engine/src/window/provider/knowdb.rs`

---

## Phase 3：wfusion.toml 配置支持

**目标**：通过 `wfusion.toml` 声明 Provider 窗口。

**任务**：
- [ ] `[window.X]` 增加 `table`、`query`、`refresh` 字段（可选，有默认值）
- [ ] bootstrap 时解析 knowdb.toml，为每个 `[window.X]` 创建 ProviderWindow
- [ ] 自动检测：knowdb.toml 不存在 → 跳过，向后兼容
- [ ] 验证：table 必须指向 knowdb.toml 中存在的表

**产出**：`wf-config` 配置扩展 + `wf-runtime` bootstrap 逻辑

---

## Phase 4：Join 下推

**目标**：ProviderWindow 的 join 查询推给 SQL，不全量拉数据。

**任务**：
- [ ] `ProviderWindow::query_filtered(conds, event) → Option<Row>`：根据 join 条件构建 SQL
- [ ] `execute_joins` 中对 ProviderWindow 走 `query_filtered`，BufferWindow 保持 `snapshot`
- [ ] ANTI join 优化：`SELECT 1 ... LIMIT 1`，只查存在性
- [ ] 性能测试：10K/100K/1M 白名单，对比全量加载 vs 按需查询

**产出**：`wf-engine/src/match_engine/executor/context.rs` 改动

---

## Phase 5：迁移当前实现

**目标**：删除 `load_knowledge_into_windows`，全部走 ProviderWindow。

**任务**：
- [ ] `port_scan_whitelist` 示例改用 ProviderWindow
- [ ] 删除 `bootstrap.rs` 中 CSV 直接加载逻辑
- [ ] 清理 `build_record_batch_from_json` pub(crate) 暴露
- [ ] 回归测试：所有示例 + wfl test 通过

**产出**：代码简化，统一入口

---

## 依赖关系

```
Phase 1 ──→ Phase 2 ──→ Phase 3 ──→ Phase 4
                              │
                              └──→ Phase 5
```

Phase 2 依赖 wp-knowledge（已有依赖）。
