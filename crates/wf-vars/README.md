# wf-vars

`wf-vars` provides a shared configuration variable model for the `wp-reactor` workspace.

It focuses on three related tasks:

- build a stable variable context from explicit vars and environment fallbacks
- expand TOML string fields using `$VAR` and `${VAR:default}` syntax
- track where final values came from, including mixed file/cli/env/default provenance

## Main Types

- `ConfigVarContext`: explicit vars + environment snapshot
- `SourceAtom`: one provenance atom such as file, cli, env, or default
- `TracedValue`: a resolved string plus its provenance set
- `ExpandedToml`: expanded TOML plus per-path provenance

## Simple APIs

- `resolve_toml_vars`
- `resolve_value_vars`
- `expand_toml`
- `expand_value`

These are the default entry points when you only care about final values.

## Provenance APIs

- `resolve_value_vars_with_sources`
- `resolve_toml_vars_with_sources`
- `expand_value_with_sources`
- `expand_toml_with_sources`
- `preprocess_toml`
- `render_source_label`

Use these when you also need final-value provenance.

## Scope

`wf-vars` itself does not define workspace-specific names such as `CONFIG_DIR` or `WORK_DIR`.

Those names should be introduced by higher-level loaders like `wf-config`, then passed into
`wf-vars` as ordinary file-local values before expansion.

## Example

```rust
use std::collections::HashMap;

use toml::Value as TomlValue;
use wf_vars::{ConfigVarContext, expand_value};

let value: TomlValue = toml::from_str(
    r#"
sinks = "${CASE_PATH}/sinks"
"#,
) ?;

let mut explicit = HashMap::new();
explicit.insert("CASE_PATH".to_string(), "/tmp/case".to_string());
let ctx = ConfigVarContext::from_explicit_vars(explicit);

let expanded = expand_value(&value, &ctx)?;

assert_eq!(
    expanded.get("sinks").and_then(TomlValue::as_str),
    Some("/tmp/case/sinks")
);
# Ok::<(), wf_vars::VarsError>(())
```
