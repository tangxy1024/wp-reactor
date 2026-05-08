# Changelog

## [0.1.3 Unreleased]

### Added

- **wf-core**: Added `CoreReason` and `CoreResult<T>` as the structured error boundary for core APIs.
- **wf-runtime**: Added `RuntimeReason` and `RuntimeResult<T>` for runtime lifecycle, receiver, metrics, tracing, schema, sink, and task boundaries.
- **wf-config**: Added `ConfigReason` and `ConfigResult<T>` for configuration loading, validation, path resolution, and sink configuration errors.
- **wf-lang**: Added `LangReason` and `LangResult<T>` for parser, validator, and compiler entry points.
- **wf-vars**: Added `VarsReason` and `VarsResult<T>` for variable expansion and resolution APIs.
- **wf-engine**: Added `EngineReason` and `EngineResult<T>` for the CLI boundary.

### Changed

- **Workspace**: Upgraded `orion-error` to `0.8.1` and adopted `#[derive(OrionError)]` reason enums with stable identities.
- **Workspace**: Removed `anyhow` from workspace and crate manifests used by `wf-core`, `wf-runtime`, and `wf-engine`.
- **wf-engine**: CLI failures now render structured `DiagnosticReport` output directly instead of converting runtime failures to unstructured errors.
- **wf-runtime**: Runtime task handles now return `RuntimeResult<()>`, preserving structured task failures through shutdown and supervisor joins.
- **wf-config**: Sink configuration variable expansion now wraps lower variable errors at the sink boundary while carrying the source file path in structured context.
- **wf-config**: Configuration APIs now use structured conversion paths (`source_err`, `source_raw_err`, and `conv_err`) instead of ad hoc string wrapping.
- **wf-lang**: Parser and compiler APIs now return structured errors while keeping parser-combinator internal error handling local to the parser.
- **wf-vars**: Variable expansion APIs now return structured errors with explicit resolve, template, and TOML reasons.

### Fixed

- **wf-runtime**: Metrics HTTP response write timeouts are now reported as structured timeout errors instead of being silently ignored.
- **wf-runtime**: Supervisor shutdown failures now preserve the lower structured error source chain instead of flattening it into a string detail.
- **wf-config**: Sink defaults, connector, business route, and infra route preprocessing failures now include structured `path` context.

### Docs

- **Docs**: Updated the error-handling design notes to describe the structured error boundaries across `wf-core`, `wf-runtime`, `wf-config`, `wf-lang`, `wf-vars`, and `wf-engine`.
- **Docs**: Updated configuration variable resolution examples and dependency notes to use `ConfigResult`, `VarsError`, and `orion-error`.

## 0.1.0

### Added

- `wfusion` runtime config supports explicit `mode = "daemon" | "batch"`.
- Input sources are unified under `[[sources]]`; TCP ingress is configured as a source and no longer uses `[server]`.
- File source formats now include:
  - `ndjson`
  - `arrow_framed`
  - `arrow_ipc`
- `arrow_framed` file replay support was added for the current `wp_arrow` length-prefixed framed format.
- User documentation was reorganized into `docs/user-guide/` with topic pages:
  - `index.md`
  - `quick-start.md`
  - `language-reference.md`
  - `runtime-config.md`
  - `tooling.md`

### Changed

- Sink runtime integration now uses `wp-core-connectors`.
- Runtime output export is now record-first:
  - internal `OutputRecord` values are exported to `DataRecord` before sink dispatch
  - sink dispatch reuses the sink `send_record` path instead of a JSON-only path
- Structured runtime output now injects reserved engine fields with the `__wfu_` prefix:
  - `__wfu_id`
  - `__wfu_rule_name`
  - `__wfu_score`
  - `__wfu_entity_type`
  - `__wfu_entity_id`
  - `__wfu_origin`
  - `__wfu_close_reason`
  - `__wfu_fired_at`
  - `__wfu_emit_time`
  - `__wfu_summary`
- `yield_fields` are expanded into exported sink records alongside the fixed `__wfu_*` fields.
- `yield_fields` with array types are currently exported as compact JSON strings.

### Fixed

- Structured output export now preserves typed `yield_fields` for `ip`, `time`, and `hex` instead of degrading them to `chars`.
- Sink dispatch no longer relies on sink kind name prefixes such as `arrow-*` to decide the payload path.
- Reserved prefix conflicts are now rejected when user `yield_fields` attempt to emit fields under `__wfu_`.
- `wfgen verify` now accepts both legacy alert JSONL fields and the new structured `__wfu_*` runtime output fields.
- Close-path aggregate expressions in `score(...)` and `yield (...)` now evaluate against step context, including:
  - `count(alias)`
  - `count(step_label)`
  - `avg(alias.field)`
  - aggregate expressions nested inside `if ... then ... else ...` and builtin functions such as `concat(...)`
- Downstream `match + close` rules now aggregate intermediate float fields correctly from close-step data, so expressions such as `avg(x.__wfu_score)` and `avg(x.risk_score)` no longer collapse to `0.0`.
- When the same alias can resolve to both event-step and close-step context during close evaluation, aggregate lookup now prefers the close-step series to avoid double-counting.
- Close-path `count(alias)` and `avg/sum/min/max/first/last(alias.field)` now also work for filtered bind aliases declared in `events { ... }`, even when that alias is not used as a match step source.
- Event-path matches now process auxiliary filtered bind aliases before step-source aliases, so same-row expressions such as `count(hi)` and `avg(elevated.risk_score)` see the current row as well.
- `match`, `on each`, and `close` executor paths no longer silently drop `yield` fields when expression evaluation returns `None`; they now fail with explicit `RuleExec` errors.
- Checker validation now rejects ambiguous set-level aggregate expressions such as `avg(alias)`, `sum(alias)`, `min(alias)`, and `max(alias)`, while continuing to allow `count(alias)`.

### Docs

- User guide examples now document:
  - `arrow_framed` vs `arrow_ipc`
  - explicit file source format selection
  - structured output export semantics
  - `__wfu_*` reserved fields
  - array export behavior as JSON string
- Changelog now records the executor-side close aggregation fix, non-silent `yield` failures, and the checker restriction on ambiguous set-level aggregates.
