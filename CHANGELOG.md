# Changelog

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

### Docs

- User guide examples now document:
  - `arrow_framed` vs `arrow_ipc`
  - explicit file source format selection
  - structured output export semantics
  - `__wfu_*` reserved fields
  - array export behavior as JSON string
