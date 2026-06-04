// Submodules — merged from wf-vars crate
pub mod context;
pub mod error;
pub mod expand;
pub mod trace;

// Re-export public API
pub use context::ConfigVarContext;
pub use error::{VarsError, VarsReason, VarsResult};
pub use expand::{
    collect_active_external_sources, expand_toml, expand_toml_with_sources, expand_value,
    expand_value_with_sources, external_value_with_source, preprocess_toml, resolve_toml_vars,
    resolve_toml_vars_with_sources, resolve_value_vars, resolve_value_vars_with_sources,
};
pub use trace::{ExpandedToml, SourceAtom, TracedValue, render_source_label};

// Existing wf-config vars helpers
use std::collections::HashMap;
use std::path::Path;

use toml::Value as TomlValue;

pub(crate) fn render_scoped_var_source_label(key: &str) -> Option<String> {
    match key {
        "CONFIG_DIR" | "WORK_DIR" => Some(format!("<builtin:{key}>")),
        _ => None,
    }
}

pub(crate) fn inject_loader_scoped_vars(
    value: &TomlValue,
    path: &Path,
    work_dir: Option<&Path>,
) -> TomlValue {
    let mut next = value.clone();
    let Some(table) = next.as_table_mut() else {
        return next;
    };
    let vars_entry = table
        .entry("vars".to_string())
        .or_insert_with(|| TomlValue::Table(Default::default()));
    let Some(vars_table) = vars_entry.as_table_mut() else {
        return next;
    };
    if let Some(config_dir) = path.parent() {
        vars_table
            .entry("CONFIG_DIR".to_string())
            .or_insert_with(|| TomlValue::String(config_dir.to_string_lossy().to_string()));
    }
    if let Some(base_dir) = work_dir {
        vars_table
            .entry("WORK_DIR".to_string())
            .or_insert_with(|| TomlValue::String(base_dir.to_string_lossy().to_string()));
    }
    next
}

pub(crate) fn materialize_loader_scoped_vars(
    ctx: &ConfigVarContext,
    path: &Path,
    file_vars: &HashMap<String, String>,
    work_dir: Option<&Path>,
) -> HashMap<String, String> {
    let mut merged = file_vars.clone();
    if let Some(config_dir) = path.parent() {
        merged
            .entry("CONFIG_DIR".to_string())
            .or_insert_with(|| config_dir.to_string_lossy().to_string());
    }
    if let Some(base_dir) = work_dir {
        merged
            .entry("WORK_DIR".to_string())
            .or_insert_with(|| base_dir.to_string_lossy().to_string());
    }
    ctx.materialize_vars(&merged)
}
