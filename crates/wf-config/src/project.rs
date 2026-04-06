use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::runtime::resolve_glob;
use crate::vars::materialize_loader_scoped_vars;
use wf_vars::ConfigVarContext;

/// Load and preprocess a .wfl file with variable substitutions.
/// Variables are resolved in order: `vars` (from `--var`) first, then
/// environment variables. An error is returned only if a variable is
/// found in neither source and has no `${VAR:default}` fallback.
pub fn load_wfl(path: &Path, vars: &HashMap<String, String>) -> Result<String> {
    let ctx = ConfigVarContext::from_explicit_vars(vars.clone());
    load_wfl_with_context(path, &ctx, None)
}

/// Load and preprocess a `.wfl` file with a shared variable context.
///
/// This keeps `.wfl` variable lookup aligned with the configuration loader:
/// explicit vars and built-in context vars are materialized first, then
/// environment variables act as a final fallback for undefined identifiers.
pub fn load_wfl_with_context(
    path: &Path,
    ctx: &ConfigVarContext,
    work_dir: Option<&Path>,
) -> Result<String> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let effective_vars = materialize_loader_scoped_vars(ctx, path, &HashMap::new(), work_dir);
    let preprocessed = wf_lang::preprocess_vars_with_env(&source, &effective_vars)?;
    Ok(preprocessed)
}

/// Load all .wfs schema files matching a glob pattern.
pub fn load_schemas(patterns: &[String], base_dir: &Path) -> Result<Vec<wf_lang::WindowSchema>> {
    let mut schemas = Vec::new();
    for pattern in patterns {
        let paths = resolve_schema_glob(pattern, base_dir)?;
        for path in paths {
            let source = std::fs::read_to_string(&path)
                .with_context(|| format!("reading schema {}", path.display()))?;
            let mut parsed = wf_lang::parse_wfs(&source)
                .map_err(|e| anyhow::anyhow!("parsing {}: {e}", path.display()))?;
            schemas.append(&mut parsed);
        }
    }
    Ok(schemas)
}

/// Resolve a glob pattern for schema files. If the pattern contains glob
/// characters, use glob expansion; otherwise treat as a literal path.
fn resolve_schema_glob(pattern: &str, base_dir: &Path) -> Result<Vec<PathBuf>> {
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        resolve_glob(pattern, base_dir)
    } else {
        let path = base_dir.join(pattern);
        if path.exists() {
            Ok(vec![path])
        } else {
            anyhow::bail!("schema file not found: {}", path.display());
        }
    }
}

/// Parse `KEY=VALUE` variable assignments from CLI arguments.
pub fn parse_vars(var_args: &[String]) -> Result<HashMap<String, String>> {
    let mut vars = HashMap::new();
    for arg in var_args {
        let (key, value) = arg.split_once('=').ok_or_else(|| {
            anyhow::anyhow!("invalid --var format: expected KEY=VALUE, got '{}'", arg)
        })?;
        vars.insert(key.to_string(), value.to_string());
    }
    Ok(vars)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "wf-config-project-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("failed to create temp dir");
        dir
    }

    fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("failed to create parent dir");
        }
        std::fs::write(path, content).expect("failed to write test file");
    }

    #[test]
    fn load_wfl_with_context_uses_explicit_vars_builtins_and_env_fallback() {
        let root = make_temp_dir("wfl-context");
        let work_dir = root.join("workspace");
        let file = root.join("rules/example.wfl");
        std::fs::create_dir_all(&work_dir).expect("failed to create work dir");
        write_file(
            &file,
            "a = $THRESHOLD\nb = $CONFIG_DIR\nc = $WORK_DIR\nd = ${WF_CONFIG_PROJECT_ENV_VAR}\n",
        );

        unsafe {
            std::env::set_var("THRESHOLD", "1");
            std::env::set_var("WF_CONFIG_PROJECT_ENV_VAR", "env_value");
        }

        let mut explicit_vars = HashMap::new();
        explicit_vars.insert("THRESHOLD".to_string(), "5".to_string());
        let ctx = ConfigVarContext::from_explicit_vars(explicit_vars);
        let loaded = load_wfl_with_context(&file, &ctx, Some(&work_dir)).expect("load wfl");

        assert!(loaded.contains("a = 5"));
        assert!(loaded.contains(&format!("b = {}", file.parent().unwrap().to_string_lossy())));
        assert!(loaded.contains(&format!("c = {}", work_dir.to_string_lossy())));
        assert!(loaded.contains("d = env_value"));

        unsafe {
            std::env::remove_var("THRESHOLD");
            std::env::remove_var("WF_CONFIG_PROJECT_ENV_VAR");
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
