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
    message = fmt("{} brute force detected", fail.sip)
)
```

系统会自动注入：

- `rule_name`
- `emit_time`
- `score`
- `entity_type`
- `entity_id`
- `close_reason`

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

Yield 约束：

- 目标 window 必须存在且 `stream` 为空
- 字段须为目标 window 的子集
- 禁止手工赋值系统字段
