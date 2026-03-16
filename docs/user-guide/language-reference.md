# 语言参考

## Window Schema (`.wfs`)

Window 是 WFL 的数据抽象层，定义事件流的逻辑结构。

基本语法：

```wfs
window <名称> {
    stream = <数据流名>
    time = <时间字段>
    over = <保留时长>

    fields {
        <字段名>: <类型>
    }
}
```

字段类型：

| WFL 类型 | 底层映射 |
|----------|----------|
| `chars` | Utf8 |
| `digit` | Int64 |
| `float` | Float64 |
| `bool` | Boolean |
| `time` | Timestamp(Nanosecond) |
| `ip` | Utf8 |
| `hex` | Utf8 |
| `array/T` | List(T) |

属性说明：

- `stream`：数据流绑定；可省略，省略时该 window 只作为输出目标
- `time`：事件时间字段；`over > 0` 时必填
- `over`：保留时长；`0` 表示静态集合

带点字段名示例：

```wfs
window endpoint_events {
    stream = "endpoint"
    time = event_time
    over = 10m

    fields {
        host_id: chars
        event_time: time
        `detail.sha256`: hex
        `detail.process`: chars
    }
}
```

在 `.wfl` 中引用时使用下标形式：`alias["detail.sha256"]`。

## 检测规则 (`.wfl`)

规则结构：

```wfl
use "schema.wfs"

rule <规则名> {
    meta { ... }
    events { ... }
    match<key:duration> {
        on event { ... }
        on close { ... }
    } -> score(expr)
    entity(type, id)
    yield target (...)
}
```

也可以使用逐条无状态触发：

```wfl
rule <规则名> {
    events { ... }
    on each <alias> [where <expr>] -> score(expr)
    entity(type, id)
    yield target (...)
}
```

### `events`

```wfl
events {
    fail : auth_events && action == "failed"
    scan : fw_events
}
```

- 别名必须唯一
- window 必须在已导入 `.wfs` 中定义
- 过滤表达式支持比较、逻辑运算、`in` / `not in`

状态枚举这类条件，优先写成：

```wfl
events {
    bad : app_events && lower(status) in ("error", "failed", "failure", "timeout", "fatal", "panic", "abort")
}
```

不推荐展开成很长的 `a == x || a == y || ...`。

### `match`

```wfl
match<sip:5m> {
    on event {
        fail | count >= 3;
        scan.dport | distinct | count > 10;
    }
}
```

说明：

- key 可为空、单 key、复合 key
- 支持滑动窗口和固定窗口：`match<sip:5m:fixed>`
- 多步是顺序关系，前一步命中后才进入后一步

固定窗口示例：

```wfl
match<sip:5m:fixed> {
    on event {
        fail | count >= 3;
    }
}
```

### `on each`

```wfl
on each e where e.action == "failed" -> score(70.0)
```

说明：

- `on each` 与 `match` 互斥
- `e` 必须来自 `events`
- `where` 在单条记录上下文中求值
- 不创建 key / window instance
- 不支持 `on close`
- 不支持 `close_reason`
- 适合上游 enrichment 和逐条风险打分
- 如果上游已有 OML/投影层，纯逐条语义映射优先放 OML，WFL 保留窗口聚合与告警逻辑

典型写法：

```wfl
rule enrich_each_event {
    events {
        e : auth_events
    }

    on each e -> score(if e.action == "failed" then 70.0 else 10.0)

    entity(ip, e.sip)

    yield enriched_events (
        event_time = e.event_time,
        sip = e.sip
    )
}
```

### `on close`

用于缺失检测或 close 阶段判断：

```wfl
match<query_id:30s> {
    on event {
        req | count >= 1;
    }
    on close {
        resp && close_reason == "timeout" | count == 0;
    }
}
```

`close_reason` 可取：

- `"timeout"`
- `"flush"`
- `"eos"`

### `score`

```wfl
} -> score(70.0)
```

也可使用表达式：

```wfl
} -> score(if count(fail) > 10 then 90.0 else 70.0)
```

### `entity`

```wfl
entity(ip, fail.sip)
entity(user, login.uid)
entity(host, e.host_id)
```

### `join`

支持 `snapshot` / `asof` / `asof within`：

```wfl
join geo_lookup snapshot on sip == geo_lookup.ip
join conn_risk asof within 24h on sip == conn_risk.ip
```

- `snapshot`：取右表当前快照
- `asof`：按事件时间回看最近一条 `ts <= event_time`
- `asof within`：在指定时间范围内回看

### `yield`

```wfl
yield security_alerts (
    sip = fail.sip,
    fail_count = count(fail),
    message = fmt("{} brute force detected, risk={}", fail.sip, @score),
    risk_score = round(@score, 1)
)
```

`@score` 表示“当前规则已经计算出的最终 score 值”。

- 只允许出现在 `yield (...)` 表达式里
- 在 `yield` 中可像普通数值一样参与任意表达式，例如 `round(@score, 1)`、`concat("risk=", @score)`
- 适合把规则 score 映射成业务字段，例如 `risk_score = @score`
- 它引用的是当前规则的 score，不是上游中间记录里的 `__wfu_score`

最终 alert 记录会自动注入：

- `rule_name`
- `emit_time`
- `score`
- `entity_type`
- `entity_id`
- `close_reason`

如果 `yield` 目标是给下游继续消费的中间 window，则按中间 enriched 记录约定透传以 `__wfu_` 为前缀的系统字段。推荐依赖：

- `__wfu_score`
- `__wfu_rule_name`
- `__wfu_entity_type`
- `__wfu_entity_id`

这几个字段对下游规则可直接引用；当某个 window 被识别为中间消费目标时，编译器会自动把它们视为该 window 的可用字段，不需要在 `.wfs` 里重复声明。

中间记录默认不暴露时间类 `__wfu_*` 字段；若目标 window 定义了 `time` 列，runtime 会在 `yield (...)` 未显式赋值时自动继承输入事件时间到该列。若你需要把时间作为普通字段继续使用，应显式写进 `yield (...)`。

`yield` 里也不能手工写 `__wfu_*` 字段名；这个前缀保留给运行时中间系统字段。

若某个 `yield` 目标会被下游规则继续消费，则所有这类中间 window 必须构成无环依赖图；禁止自回写和 `A -> B -> A` 形式的循环。

### `limits`

```wfl
limits {
    max_memory = "50MB";
    max_instances = 10000;
    max_throttle = "100/min";
    on_exceed = "throttle";
}
```

`on_exceed` 可选：

- `throttle`
- `drop_oldest`
- `fail_rule`

## 表达式与函数

运算符优先级，从高到低：

1. 一元 `-`
2. `*` `/` `%`
3. `+` `-`
4. `==` `!=` `<` `>` `<=` `>=` `in` `not in`
5. `&&`
6. `||`

### 聚合函数

| 函数 | 说明 |
|------|------|
| `count(alias)` | 事件计数 |
| `sum(alias.field)` | 求和 |
| `avg(alias.field)` | 平均值 |
| `min(alias.field)` | 最小值 |
| `max(alias.field)` | 最大值 |
| `distinct(alias.field)` | 去重计数 |

示例：

```wfl
fail | count >= 3;
scan.dport | distinct | count > 10;
e.bytes | sum >= 10000;
```

这些聚合表达式可以直接引用 `events { ... }` 里声明的 alias。
包括带过滤条件、但没有出现在 `on event` / `and close` step source 里的 alias，例如 `count(hi)`、`avg(elevated.risk_score)`。

### 格式化函数

```wfl
fmt("{} failed {} times from {}", fail.username, count(fail), fail.sip)
```

### 字符串函数

| 函数 | 说明 |
|------|------|
| `contains(haystack, needle)` | 子串匹配 |
| `lower(field)` | 转小写 |
| `upper(field)` | 转大写 |
| `len(field)` | 字符串长度 |

示例：

```wfl
events {
    ps : endpoint_events && contains(lower(cmd), "powershell")
}
```

结合 `in` 可简化多值匹配：

```wfl
events {
    bad : endpoint_events && lower(status) in ("error", "failed", "failure")
}
```

## 规则测试

契约测试语法：

```wfl
test <测试名> for <规则名> {
    input {
        row(<别名>, <字段> = <值>, ...);
        tick(<时长>);
    }
    expect {
        hits == <数量>;
        hit[<索引>].score == <分数>;
        hit[<索引>].entity_id == <值>;
        hit[<索引>].field("<字段名>") == <值>;
    }
    options {
        close_trigger = timeout;
        eval_mode = strict;
    }
}
```

示例：

```wfl
test brute_test for brute_force {
    input {
        row(fail, action = "failed", sip = "1.2.3.4");
        row(fail, action = "failed", sip = "1.2.3.4");
        row(fail, action = "failed", sip = "1.2.3.4");
        tick(6m);
    }
    expect {
        hits == 1;
        hit[0].score == 70.0;
        hit[0].entity_id == "1.2.3.4";
    }
    options {
        close_trigger = timeout;
    }
}
```

## 能力分层

### L1

- 基础规则结构
- `match<key:dur>`
- 多步序列
- `on close`
- OR 分支
- 聚合函数
- `yield`
- `score`
- `entity`
- `fmt`
- 变量预处理
- 字符串函数
- 固定窗口
- 规则测试

### L2

已实现：

- `join`
- `limits`
- `contains` / `lower` / `upper` / `len`

设计中：

- `baseline(expr, dur)`
- `window.has(field)`
- `derive`
- `score { ... @ weight }`
- `if/then/else`
- `regex_match`
- `time_diff` / `time_bucket`
- `coalesce` / `try`

### L3

- 多级管道
- 会话窗口
- 集合函数
- 统计函数

## 语义约束速查

Events 约束：

- 别名唯一
- window 必须存在
- 过滤字段必须存在于对应 window 中

Match 约束：

- `duration > 0`
- step 必须显式声明 source
- `on event` 必选且至少一条 step
- `close_reason` 仅可在 `on close` 中引用
- `match` 与 `on each` 互斥

On Each 约束：

- `alias` 必须来自 `events`
- `where` 必须返回 `bool`
- 不支持 `close_reason`
- 不支持集合函数和窗口状态函数

Yield 约束：

- 目标 window 必须存在且 `stream` 为空
- 字段须为目标 window 的子集
- 禁止手工赋值系统字段
