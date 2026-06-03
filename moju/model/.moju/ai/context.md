# Moju AI Context

This file is context for `.moju/ai/ai-task.md`. Follow the selected AI task, not a generic fix task.

## Project
/Users/zuowenjian/devspace/rust/wfusion/wp-reactor/moju/model

## Active View
Flows

## Active Domain
Orchestra

## Selected Element
Flow `Orchestra.Run`

## Model Summary
121 structs, 13 flows, 19 modules, 8 verify cases

## Diagnostics
- check info: OK - 121 structs, 13 flows, 19 modules, 8 verify cases

## Related Files
- /Users/zuowenjian/devspace/rust/wfusion/wp-reactor/moju/model/domain/orchestra/domain.mju

## Source Snippets
### /Users/zuowenjian/devspace/rust/wfusion/wp-reactor/moju/model/domain/orchestra/domain.mju

```mju
// ---------------------------------------------------------------------------
// orchestra domain — CLI entry, lifecycle, task orchestration, adapters
// Crate: wf-runtime
// ---------------------------------------------------------------------------

// -- Actors -------------------------------------------------------------------

actor Admin {
  can RunCli
  can RenderConfig
  can DiffConfig
}

// -- CLI trigger commands ----------------------------------------------------

command RunCli {
  config
  overlay
  var
  work_dir
}

command RenderConfig {
  config
}

command DiffConfig {
  base_config
  to_config
}

// -- Runtime trigger commands ------------------------------------------------

command RunCommand {
  config_path
  work_dir
}

command ShutdownSignal {
  reason
}

command ConfigChange {
  new_config
}

command TimerTick {
  rule_name
}

// -- CLI types ---------------------------------------------------------------

struct RunArgs {
  config
  overlay
  var
  work_dir
  metrics
  metrics_interval
  metrics_listen
}

state Command {
  Run
  Config
}

state ConfigCommands {
  Render
  Origins
  Vars
  Diff
}

struct RenderArgs {
  raw
}

struct PathFilterArgs {
  path_prefix
}

struct VarFilterArgs {
  var_prefix
}

struct DiffArgs {
  to_config
  to_overlay
  to_var
  to_work_dir
  path_prefix
  expanded
}

struct ConfigLoadArgs {
  config
  overlay
  var
  work_dir
}

// -- Reactor lifecycle -------------------------------------------------------

struct Reactor {
  config
  cancel_token
  listen_addr
}

struct BootstrapData {
  rules
  dispatcher
  router
  schemas
}

struct RunRule {
  plan
  kind
}

state RunRuleKind {
  UserRule
  PipelineInternal
}

state ShutdownTrigger {
  Signal
  Internal
}

// -- Task orchestration ------------------------------------------------------

struct TaskGroup {
  name
  tasks
}

// -- Reload ------------------------------------------------------------------

struct PreparedRuleReload {
  new_rules
  old_rule_names
}

state ReloadPreparation {
  Ready
  Blocked
}

// -- Receiver ----------------------------------------------------------------

struct Receiver {
  config
  bind_addr
}

// -- Sink factory ------------------------------------------------------------

struct SinkFactoryRegistry {
  factories
}

// -- Metrics -----------------------------------------------------------------

struct RuntimeMetrics {
  events_total
  events_per_window
  rule_matches
  sink_errors
  histogram
}

struct Histogram {
  buckets
}

struct HistogramSnapshot {
  counts
}

struct IntervalRates {
  events_per_sec
  matches_per_sec
}

struct IntervalSnapshot {
  counts
  rates
}

struct TotalCounts {
  events
  matches
  errors
}

struct RunSummary {
  rule_name
  matches
  avg_latency_us
}

// -- Error state ------------------------------------------------------------

state EngineReason {
  Cli
  Config
  Runtime
  General
}
```

## Working Rules
- Use `.moju/ai/ai-task.md` as the task source.
- Keep changes focused on relevant `.mju`, `layout.json`, or necessary documentation files.
- Do not introduce duplicate definitions.
- Run the relevant `moju verify .` / `moju readiness .`, or the project's existing validation command.
