# Changelog

## [0.1.19] — 2026-06-24

### Documentation

- **WFG**: Moved the canonical WFG design document to
  `warp-fusion/docs/design/wfg-design.md`, where `wfgen` is implemented, and
  left `wp-reactor/docs/design/wfg-design.md` as a pointer to the migrated
  document.
- **WFL/WFG design alignment**: Updated design notes to distinguish
  `wp-reactor` library capabilities from the CLI/tooling implemented in the
  sibling `warp-fusion` repository, including `wfl test/replay/verify` and
  `wfgen` generation/verification workflows.
- **WFG syntax**: Aligned the WFG design with current `wfgen` parser behavior
  (`with(count)`, optional `for RULE`, `then use(...)`, extended `expect`
  metrics) and marked `not(...) within(...)` as parser-supported but not yet
  datagen-supported.

## [0.1.17]

### Added

- **wf-lang**: `collect_bind_tracking_aliases` now collects aliases from plain
  `Expr::Field(FieldRef::Qualified/Bracketed)` expressions. Previously only
  series functions (e.g. `count(e)`) contributed to `tracked_bind_aliases`;
  now `e.dip` in yield expressions correctly adds alias `e` to the set.
- **wf-engine**: `build_eval_context` now exposes bind_data and step_data
  field values as plain field names (e.g. `dip`) in addition to the existing
  prefixed keys (`_bind_e_field_dip`, `_step_0_field_dip`). This allows
  yield expression evaluators that look up fields by plain name to find them.
- **wf-lang tests**: 4 new unit tests for `collect_bind_tracking_aliases`
  covering qualified/bracketed/simple field refs and full yield expressions.
- **wf-engine tests**: 1 new unit test for `build_eval_context` verifying
  plain field name exposure from bind_data.

### Changed

- **wf-lang**: `collect_rule_bind_tracking_aliases` and
  `collect_bind_tracking_aliases` visibility changed `fn` → `pub(crate)` for
  testability.

### Fixed

- **wf-engine**: Fixed 2 pre-existing clippy warnings in
  `match_engine/tests/l2/expr.rs` (collapsible-if + let-unit-value).


## [0.1.15] — 2026-06-18

### Added

- **wf-runtime**: `DataSourceBatchSource` adapter (`wf-runtime/src/source/mod.rs`) —
  bridges `wp_connector_api::DataSource` to `wf_connector_api::BatchSource`,
  handling Arrow IPC / NDJSON / wp_arrow framed decode and EOF mapping.
- **wf-runtime**: `ArrowFramed` format extracts the wp_arrow frame tag and uses
  it as the routing stream name when no explicit `stream` param is configured.
- **wf-engine**: `external()` WFL function support — `ExternalCallHandler` trait
  + global `dispatch_external_call` + `eval_external` shared helper
  (`wf-engine/src/external.rs`). Both eval paths (executor + match_engine)
  route `external()` to the global handler.
- **wf-runtime**: `ExternalRuntime` + `RedisBackend` (`wf-runtime/src/external/`)
  — bridges WFL `external()` calls to `wp_knowledge::facade` (Redis backend).
  Bootstrap installs the handler and initializes Redis from `knowdb.toml`.
  Error handling: `external_exists` returns `Bool(false)` on Redis failure
  (fail-closed); `external_value` returns `None`.

### Changed

- **wf-connector-api integration landed.** Runtime now consumes source data
  via the `BatchSource` trait, replacing inline Arrow IPC / NDJSON decode logic
  in `spawn_external_source_tasks`.
- **`wp-core-connectors` upgraded 0.5.0 → 0.5.2.** Source factories validate
  `data_format` in `validate_spec`; `WireFormat` enum
  (`Ndjson` / `ArrowStream` / `ArrowFramed`) replaces the runtime's custom
  `SourceFormat`. Decode logic delegates to connector-layer shared helpers
  (`decode_arrow_ipc_batches` / `decode_arrow_framed_batches`).
- **`wp-connectors` upgraded v0.15.4 → v0.15.5.**
- **Config parameter renamed:** `format` → `data_format` for source payload
  format declaration (TCP / file / syslog sources).
- **Removed `listen_addr` from `Reactor`.** TCP listen address is a connector
  implementation detail, not tracked at the Reactor level. `spawn_receiver_task`
  now returns `TaskGroup` instead of `(Option<SocketAddr>, TaskGroup)`.
- **Removed dead `Receiver` struct** and inline TCP handler
  (`handle_connection` / `handle_connection_stream` / `read_frame` for TCP)
  from `receiver.rs`. Production code uses the connector factory path.
- **`wf-config`**: file source validation now reads `data_format` instead of
  `format`.
- **`wf-runtime`**: file source replay path now reads `data_format` instead of
  `format`.

### Fixed

- **EOF handling.** `DataSource` returning `SourceReason::EOF` no longer
  causes infinite retry loops — the source task exits cleanly when the stream
  ends.
- **Schema resolution.** Arrow formats (`ArrowFramed` / `ArrowStream`) skip
  pre-resolved window schema at startup — the schema is embedded in the IPC
  stream itself, so resolving from a (possibly empty) stream name was
  incorrect.
- **`external()` error handling.** `call_bool` returning `Ok(None)` (exists=false)
  now directly returns `Bool(false)` instead of incorrectly falling through to
  `call_value`. Previously, "password not in weak-password DB" would trigger a
  spurious HGET query.
- **`external()` code dedup.** Both eval paths now share `eval_external()`
  helper instead of duplicating arg-parsing logic.
- **`external()` test pollution.** `OnceLock` global handler test adjusted to
  avoid cross-test state leakage.

### Documentation

- `docs/source-architecture.md` rewritten to reflect the three-layer
  architecture (connector SourceFactory + WireFormat + BatchSource).
- `docs/user-guide/runtime-config.md` updated for connector-based TCP source
  params (`addr` / `port` / `framing` / `data_format`).
- `docs/design/arrow-tcp-stream-compatibility.md` marked as implemented.
- `docs/design/warp-fusion.md` Reactor struct updated (removed `listen_addr`).
- `docs/design/external-function-design.md` §6.1 error handling updated with
  full dispatch logic; §10 Phase 0 implementation details updated; §11.1
  known P0 limitations table added (L1-L5).
- All example TOML configs updated `format` → `data_format`.

## [0.1.12] — 2026-06-15

### Added

- **wf-runtime**: Added external source startup through the `wp-core-connectors` source factory registry, including builtin `file`, `tcp`, and `syslog` source factory registration.
- **wf-runtime**: Added global sink factory import support so application-registered `wp-core-connectors` sink factories are available during sink dispatcher bootstrap.

### Changed

- **Workspace**: Kept `wp-core-connectors` on the published `0.5` crate dependency and added `async-broadcast` for source acceptor lifecycle control.
- **wf-runtime**: External source ingestion now reuses WFS-to-Arrow schema resolution before routing decoded NDJSON payloads.
- **wf-runtime**: External source parameters are converted to typed JSON values (`bool`, integer, float, or string) before factory validation/build.

### Fixed

- **wf-config**: Batch mode validation again rejects enabled non-file sources, preventing daemon-style receivers from starting in batch runs.
- **wf-config**: Enabled external sources now require a non-empty `stream` so schema subscription failures are caught during configuration validation.
- **wf-runtime**: Unknown external source kinds now fail bootstrap with a clear error instead of being silently skipped.
- **wf-runtime**: External source decode and route failures are now logged and reflected in receiver decode / route error metrics.
- **wf-runtime**: Source acceptors now receive a `ControlEvent::Stop` on runtime cancellation.

## [0.1.7] — 2026-06-12

### Added

- **wp-core-connectors**: File and TCP sinks now support Arrow output via `protocol = "arrow"`.
- **wp-core-connectors**: Arrow file sinks support append mode and optional `sync` fsync for durability.
- **wp-core-connectors**: Arrow TCP sink supports automatic reconnect with exponential backoff.

### Changed

- **wp-core-connectors**: Arrow sink configuration consolidated under `protocol` dispatch (`"arrow"` / `"txt"`).

### Fixed

- **wp-core-connectors**: Invalid `protocol` values now produce a clear configuration error instead of silently defaulting to text mode.

## [0.1.3] — 2025-11-15

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
