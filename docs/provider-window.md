# Provider Window 设计

## 目标

统一所有外部数据源（CSV、SQLite、Postgres）的接入方式，规则层通过 `join` 透明访问。

## 架构

```
┌──────────────────────────────────────────────────────┐
│ WFL Rule                                              │
│   join scanner_whitelist anti on e.sip == wl.sip     │
└──────────────────────┬───────────────────────────────┘
                       │ WindowLookup::snapshot("scanner_whitelist")
┌──────────────────────▼───────────────────────────────┐
│ WindowRegistry                                        │
│                                                       │
│  ┌──────────────────┐  ┌───────────────────────────┐ │
│  │ BufferWindow     │  │ ProviderWindow             │ │
│  │ (事件流窗口)      │  │ (外部数据窗口)              │ │
│  │                  │  │                            │ │
│  │ in-memory buffer │  │ provider: "knowdb"         │ │
│  │ stream → events  │  │ table: "scanner_whitelist" │ │
│  │ over = 30m       │  │ refresh: "5m"              │ │
│  └──────────────────┘  │ cache: RowData[]           │ │
│                        └──────────┬────────────────┘ │
│                                   │                   │
└───────────────────────────────────┼───────────────────┘
                                    │ facade::query()
┌───────────────────────────────────▼───────────────────┐
│ wp-knowledge facade                                    │
│                                                       │
│  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │
│  │ CSV      │  │ SQLite   │  │ Postgres            │  │
│  │ (开发)    │  │ (默认)   │  │ (生产)              │  │
│  └──────────┘  └──────────┘  └────────────────────┘  │
└───────────────────────────────────────────────────────┘
```

## 窗口类型

### BufferWindow（现有）
数据从事件流进入，append-only，有 watermark 驱逐。

### ProviderWindow（新增）
数据从外部源加载，本地缓存，定期刷新。

```
window scanner_whitelist {
    provider = "knowdb"     # 数据源类型
    table = "scanner_whitelist"
    refresh = "5m"          # 缓存刷新间隔（0 = 不刷新）
    over = 0                # 静态窗口
    fields { sip: ip, note: chars }
}
```

## 数据加载策略

### 全量加载（适合小表，< 10K 行）

启动时全量读入本地缓存，定时刷新。

```
bootstrap → facade::query("SELECT * FROM scanner_whitelist") → 缓存 → WindowLookup
```

### 按需查询（适合大表，> 10K 行）

join 时根据查询条件只取需要的行。

```
execute_joins → join condition: e.sip == wl.sip
             → facade::query("SELECT * FROM scanner_whitelist WHERE sip = ?", [e.sip])
             → 只返回匹配行
```

## Join 下推

当前 join 在引擎侧遍历所有行做匹配。Provider 窗口可以把条件推给 SQL：

```rust
// 当前：拉全部数据到内存匹配
let rows = windows.snapshot("scanner_whitelist"); // 返回所有行
let matched = find_matching_row(&rows, &join.conds, ctx); // 内存里遍历

// 优化：条件推给 SQL
let matched = windows.query_join("scanner_whitelist", &join.conds, ctx);
// → SELECT * FROM scanner_whitelist WHERE sip = '10.0.2.1'
```

| 策略 | 适用 | 数据量 |
|------|------|--------|
| 全量加载 | 白名单、配置表 | < 10K |
| 按需查询 | 威胁情报、IP 库 | > 10K |
| Join 下推 | 大规模、需要过滤 | 不限 |

## 缓存层级

```
join 查询
  ↓
ProviderWindow 本地缓存 (HashMap<K, V>, refresh 控制)
  ↓ 未命中
facade 查询缓存 (LRU, ttl_ms 控制)
  ↓ 未命中
SQLite / Postgres
```

| 缓存层 | 配置 | 作用 |
|--------|------|------|
| 窗口本地缓存 | `refresh = "5m"` | 定时清空，下次 join 重新触发查询 |
| facade LRU | `ttl_ms = 300000` | 相同 SQL 参数 5 分钟内直接返回，不查库 |

对于白名单（4 条记录，静态）：第一个事件触发 1 次 SQL，之后全部命中本地 HashMap，0 次 IO。

## 实现步骤

1. `ProviderWindow` 类型：实现 `WindowLookup` trait，内部持有 `Arc<dyn Provider>`
2. `knowdb provider`：封装 `facade::query()`，支持缓存 + 刷新
3. `csv provider`：当前 CSV 直接读的逻辑封装为 provider
4. `join 下推`：`find_matching_row` 改为调用 `provider.query_filtered()`
5. `wfusion.toml` 配置：`[window.X]` 增加 `provider` 字段

## 与当前实现的关系

```
当前: knowdb.toml → bootstrap 全量读入 BufferWindow（一次性快照）
目标: knowdb.toml → ProviderWindow（按需查询 + 缓存 + 自动刷新）
```

当前实现是目标的第一步——验证了 knowdb → window 的可行性。后续换 `ProviderWindow` 即可。
