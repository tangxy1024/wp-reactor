# 配置变量统一机制设计
<!-- 状态：Draft for review | 创建：2026-03-16 -->

## 1. 背景

当前 `wp-reactor` 内部已经出现了多条配置变量处理链路：

- `wfusion.toml` 支持 `[vars]` 和环境变量回退
- `sinks/` 目录中的 connector / route / defaults TOML 也开始支持 `${VAR}`
- `.wfl` 通过 `wf_lang::preprocess_vars_with_env(...)` 使用变量
- CLI 还有 `--var KEY=VALUE` 形式的注入能力

这些能力在短期修复层面已经可用，但长期仍存在结构性问题：

1. 变量来源和优先级尚未形成统一模型
2. base config / overlay config / sink config 的 merge 与 resolve 尚未分层
3. 路径解析基准同时涉及 `config dir`、`work_dir`、`work_root`，语义容易漂移
4. 当前实现仍以“读文件后做 TOML 字符串预处理”为主，适合作为过渡方案，不适合作为长期主架构

本设计的目标，是为后续配置系统统一化提供稳定边界。

## 2. 目标

### 2.1 目标

1. 定义统一的变量上下文模型
2. 明确 `load -> merge -> resolve -> deserialize -> validate` 的处理顺序
3. 统一 `wfusion.toml`、`sinks/*.toml`、`.wfl` 的变量来源与优先级
4. 固化相对路径、绝对路径、`work_dir`、`work_root` 的语义
5. 为后续支持 overlay / change config / hot reload 预留结构

### 2.2 非目标

1. 本阶段不引入新的配置文件格式
2. 本阶段不实现完整 overlay 系统
3. 本阶段不把所有字符串字段都改造成显式模板类型
4. 本阶段不把 `wf-config` 直接绑定到 `EnvDict`

## 3. 设计原则

### 3.1 merge 和 resolve 必须分开

配置覆盖和变量展开是两类不同问题：

- merge 解决“谁覆盖谁”
- resolve 解决“字符串里的引用如何求值”

如果把两者混在一起，就会出现这类不稳定语义：

- 先展开 base 再 merge overlay
- 先 merge raw 再展开
- overlay 自己的 `[vars]` 是否能覆盖 base 的 `[vars]`

长期方案必须明确：

`raw merge` 先于 `variable resolve`

### 3.2 环境变量不是核心模型

环境变量只是变量来源之一，不应该成为配置系统的主结构。

这意味着：

- 不引入“全局到处传递的环境字典对象”作为核心配置模型
- `wf-config` 内部不直接以 `EnvDict` 作为一等抽象
- 真正的一等抽象应是“解析上下文”

### 3.3 路径解析晚于变量展开

必须先完成：

- `${CASE_PATH}`
- `${WORK_DIR}`
- `${WORK_ROOT}`

等变量替换，再做路径 absolutize。否则相对路径和变量拼接后的语义会错乱。

### 3.4 默认语义优先于魔法覆盖

长期要避免这种语义：

- “有时相对 config dir”
- “有时相对当前 shell cwd”
- “有时相对 work_dir”

推荐固定规则：

1. 绝对路径：原样使用
2. 相对路径：默认相对当前配置文件所在目录
3. `work_dir` / `work_root`：只作为显式变量或显式解析基准参与，不隐式替换所有路径规则

## 4. 统一处理管线

长期配置处理统一为以下 6 个阶段：

```text
1. load source files
2. parse raw values
3. merge raw config trees
4. resolve variables against a shared context
5. deserialize into typed config
6. validate + absolutize paths
```

更具体地：

```text
load
  -> RawConfigTree
  -> merge(base, overlay, cli_override)
  -> resolve(raw, ctx)
  -> deserialize<T>()
  -> absolutize_paths(base_dir, work_dir, work_root)
  -> validate()
  -> FinalConfig
```

### 4.1 为什么 resolve 放在 deserialize 之前

优点：

1. 对 `wfusion.toml` / `sinks/*.toml` 这类 TOML 配置最直接
2. 对 overlay merge 更自然
3. 对 `${VAR}` / `${VAR:default}` 这种文本占位符最容易实现

缺点：

1. 无法天然知道“哪些字符串字段允许模板”
2. 错误容易停留在文本层而非字段语义层

因此长期建议是：

- 短中期：保留 raw-stage resolve
- 中长期：逐步把重要字段迁移到 typed-stage resolve

也就是“双层 resolve”模型：

- raw-stage resolve：保证统一变量来源和 merge 顺序
- typed-stage resolve：保证字段级语义约束

## 5. 统一变量上下文

建议引入统一上下文：

```rust
pub struct ConfigVarContext {
    pub explicit_vars: std::collections::HashMap<String, String>,
    pub file_vars: std::collections::HashMap<String, String>,
    pub env_vars: std::collections::HashMap<String, String>,
    pub config_dir: std::path::PathBuf,
    pub work_dir: Option<std::path::PathBuf>,
    pub work_root: Option<std::path::PathBuf>,
}
```

说明：

- `explicit_vars`
  来自 CLI `--var KEY=VALUE` 或调用方显式注入
- `file_vars`
  来自当前配置文件 `[vars]`
- `env_vars`
  来自进程环境变量快照
- `config_dir`
  当前正在解析的配置文件目录
- `work_dir`
  运行时显式指定的 CLI `--work-dir`
- `work_root`
  配置文件中的项目根概念

### 5.1 当前实现优先级

当前已经落地的优先级如下：

1. `explicit_vars`
2. `file_vars`
3. builtin context vars (`CONFIG_DIR` / `WORK_DIR` / `WORK_ROOT`)
4. `env_vars`
5. `${VAR:default}` 默认值

### 5.2 overlay 下的 `[vars]`

overlay merge 发生在 variable resolve 之前，因此：

- 后续 overlay 的 `[vars]` 会覆盖前面的 `[vars]`
- merge 完成后才统一进入 resolve
- 不会出现“base 先展开、overlay 再覆盖”的不稳定行为

按当前落地语义，可等价理解为：

1. `explicit_vars`
2. merge 后的最终 `[vars]`
3. builtin context vars
4. `env_vars`
5. `${VAR:default}`

这条顺序必须在以下位置保持一致：

- `wfusion.toml`
- `sinks/*.toml`
- `.wfl`
- 未来的 overlay / hot reload

## 6. 路径语义

### 6.1 默认规则

对路径字段采用统一语义：

1. 绝对路径：直接使用
2. 相对路径：相对当前配置文件所在目录

适用字段包括但不限于：

- `runtime.schemas`
- `runtime.rules`
- `sinks`
- `[[sources]].path`
- `logging.file`
- sink connector / route params 中的路径字段

### 6.2 `--work-dir`

`--work-dir` 的长期定位不应该是“偷偷改变所有字段的默认相对路径基准”，而应是：

1. 一个显式的运行时上下文输入
2. 一个可被 `${WORK_DIR}` 引用的变量
3. 在 CLI 层对“运行基准目录”的显式 override

短期内，CLI 可以继续把它作为相对路径解析基准覆盖 `config dir` 行为。
长期建议逐步收敛为：

- 默认仍以 `config dir` 为准
- 若用户需要项目根语义，则显式写 `${WORK_DIR}` 或 `${WORK_ROOT}`

### 6.3 `work_root`

`work_root` 的长期含义建议固定为：

- “项目工作根目录”
- 面向 sink / output / project layout 的业务语义

它不应该和 `config dir` 混为一谈，也不应该自动成为所有路径字段的隐式基准。

## 7. 不同输入类型的统一策略

### 7.1 `wfusion.toml`

走完整管线：

```text
raw parse -> raw merge -> resolve(ctx) -> typed config -> validate
```

### 7.2 `sinks/*.toml`

与 `wfusion.toml` 使用同一变量上下文与优先级。

长期建议也按 raw tree 处理：

```text
raw parse -> resolve(ctx) -> typed sink config -> validate
```

### 7.3 `.wfl`

`.wfl` 不属于 TOML 配置，但变量来源必须共享同一上下文。

建议规则：

- `.wfl` 继续使用文本级预处理
- 变量查找顺序与 `ConfigVarContext` 保持一致
- 不再让 `.wfl` 单独定义另一套优先级

换句话说：

`.wfl` 可以保留“文本预处理实现”，但不能保留“独立变量模型”

## 8. 接口草案

### 8.1 原始配置 merge

```rust
pub trait RawMerge {
    fn merge(self, overlay: Self) -> Self;
}
```

### 8.2 变量解析

```rust
pub trait ConfigResolve: Sized {
    fn resolve(self, ctx: &ConfigVarContext) -> anyhow::Result<Self>;
}
```

### 8.3 路径解析

```rust
pub trait PathAbsolutize: Sized {
    fn absolutize(
        self,
        config_dir: &std::path::Path,
        work_dir: Option<&std::path::Path>,
        work_root: Option<&std::path::Path>,
    ) -> anyhow::Result<Self>;
}
```

### 8.4 顶层装配接口

```rust
pub struct ConfigLoader;

impl ConfigLoader {
    pub fn load_fusion(
        path: &std::path::Path,
        overlay_paths: &[std::path::PathBuf],
        explicit_vars: &std::collections::HashMap<String, String>,
        work_dir: Option<&std::path::Path>,
    ) -> anyhow::Result<FusionConfig> {
        todo!()
    }
}
```

## 9. 迁移策略

### Phase 1

- 保留当前 `preprocess_toml(...)`
- 抽出统一 `ConfigVarContext`
- 让 `wfusion.toml` / `sinks/*.toml` / `.wfl` 使用一致优先级

### Phase 2

- 引入 raw merge
- 支持 base + overlay / change config
- 让 `--work-dir` 进入 `ConfigVarContext`

### Phase 3

- 为关键字段引入 typed-stage resolve
- 收缩对“全文字符串预处理”的依赖
- 明确哪些字段允许模板、哪些字段不允许模板

## 10. 当前实现与长期方案的关系

当前仓库中的实现可以视为 Phase 1 的过渡版本：

- 已统一 `wfusion.toml` 与 `sinks/*.toml` 的 `${VAR}` 展开能力
- 已支持环境变量回退
- 已引入 `--work-dir`

但它仍有这些局限：

1. 以 TOML 字符串预处理为主
2. 还没有 raw merge 抽象
3. `work_dir` 仍主要通过 CLI 直接影响解析基准
4. `.wfl` 还没有显式接入统一 `ConfigVarContext`

因此，后续设计和实现工作应优先落在：

- 统一 context
- 统一优先级
- 统一 raw merge
- 再逐步减少全文本预处理的职责

## 11. 评审问题

当前建议优先确认以下问题：

1. `explicit_vars > file_vars > env_vars > default` 是否作为全系统固定优先级
是的
2. `--work-dir` 长期是否降级为显式上下文变量，而不再覆盖默认 `config dir`
是的
3. overlay / change config 的 `[vars]` 是否允许覆盖 base `[vars]`
是的
4. 是否要为路径字段引入显式新类型，例如 `PathTemplate` / `GlobTemplate`
看实现的合理性
5. `.wfl` 是否接受统一接入 `ConfigVarContext`，而不再单独定义变量来源规则
统一。
