use std::collections::HashMap;
use std::path::Path;
use toml::Value as TomlValue;
use crate::vars::ConfigVarContext;

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
