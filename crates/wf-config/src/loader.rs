use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use toml::Value as TomlValue;
use wf_vars::{
    ConfigVarContext, SourceAtom, collect_active_external_sources, expand_toml,
    expand_value_with_sources, external_value_with_source, render_source_label,
    resolve_value_vars_with_sources,
};

use crate::fusion::FusionConfig;

#[derive(Debug, Clone)]
pub struct RawFusionConfigTree {
    value: TomlValue,
    origins: HashMap<String, PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RawFusionConfigChange {
    pub path: String,
    pub old_value: Option<TomlValue>,
    pub new_value: Option<TomlValue>,
    pub old_origin: Option<PathBuf>,
    pub new_origin: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedConfigVar {
    pub key: String,
    pub value: String,
    pub source: String,
}

impl RawFusionConfigTree {
    pub fn new(value: TomlValue, source_path: &Path) -> Self {
        let mut origins = HashMap::new();
        record_origins(&value, source_path, None, &mut origins);
        Self { value, origins }
    }

    pub fn value(&self) -> &TomlValue {
        &self.value
    }

    pub fn to_toml_string(&self) -> anyhow::Result<String> {
        Ok(toml::to_string(&self.value)?)
    }

    pub fn origin_for(&self, path: &str) -> Option<&Path> {
        self.origins.get(path).map(PathBuf::as_path)
    }

    pub fn origin_entries(&self) -> Vec<(String, PathBuf)> {
        let mut entries: Vec<(String, PathBuf)> = self
            .origins
            .iter()
            .map(|(path, origin)| (path.clone(), origin.clone()))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries
    }

    pub fn diff(&self, next: &RawFusionConfigTree) -> Vec<RawFusionConfigChange> {
        let mut changes = Vec::new();
        diff_value(
            Some(&self.value),
            Some(&next.value),
            "",
            self,
            next,
            &mut changes,
        );
        changes
    }

    pub fn merge_overlay(&mut self, overlay: RawFusionConfigTree) {
        merge_value_with_origins(
            &mut self.value,
            &mut self.origins,
            overlay.value,
            &overlay.origins,
            None,
        );
    }

    fn refresh_origins(&mut self, source_path: &Path) {
        self.origins.clear();
        record_origins(&self.value, source_path, None, &mut self.origins);
    }
}

pub struct FusionConfigLoader<'a> {
    base_path: &'a Path,
    overlay_paths: &'a [PathBuf],
    ctx: &'a ConfigVarContext,
}

impl<'a> FusionConfigLoader<'a> {
    pub fn new(
        base_path: &'a Path,
        overlay_paths: &'a [PathBuf],
        ctx: &'a ConfigVarContext,
    ) -> Self {
        Self {
            base_path,
            overlay_paths,
            ctx,
        }
    }

    pub fn load(&self) -> anyhow::Result<FusionConfig> {
        let merged = self.load_raw()?.to_toml_string()?;
        let file_ctx = self.ctx.for_file(self.base_path);
        FusionConfig::from_toml_with_context(&merged, &file_ctx)
    }

    pub fn load_raw(&self) -> anyhow::Result<RawFusionConfigTree> {
        let mut merged = read_toml_file(self.base_path)?;
        let base_dir =
            canonicalize_existing_dir(self.base_path.parent().ok_or_else(|| {
                anyhow::anyhow!("base config path must have a parent directory")
            })?)?;
        let target_base_dir = match self.ctx.work_dir() {
            Some(work_dir) => canonicalize_existing_dir(work_dir)?,
            None => base_dir,
        };

        for overlay_path in self.overlay_paths {
            let overlay_dir =
                canonicalize_existing_dir(overlay_path.parent().ok_or_else(|| {
                    anyhow::anyhow!("overlay path must have a parent directory")
                })?)?;
            let mut overlay = read_toml_file(overlay_path)?;
            rebase_overlay_paths(&mut overlay.value, &overlay_dir, &target_base_dir);
            overlay.refresh_origins(overlay_path);
            merged.merge_overlay(overlay);
        }

        Ok(merged)
    }

    pub fn load_merged_toml(&self) -> anyhow::Result<String> {
        self.load_raw()?.to_toml_string()
    }

    pub fn load_expanded_toml(&self) -> anyhow::Result<String> {
        let merged = self.load_raw()?.to_toml_string()?;
        let file_ctx = self.ctx.for_file(self.base_path);
        let expanded = expand_toml(&merged, &file_ctx, false)?;
        let _ = FusionConfig::from_toml_with_context(&expanded, &file_ctx)?;
        Ok(expanded)
    }

    pub fn load_expanded_raw(&self) -> anyhow::Result<RawFusionConfigTree> {
        let raw = self.load_raw()?;
        let file_ctx = self.ctx.for_file(self.base_path);
        let expanded_with_sources = expand_value_with_sources(raw.value(), &file_ctx, |path| {
            raw.origin_for(path).map(Path::to_path_buf)
        })?;
        let value = expanded_with_sources.value;
        let expanded = toml::to_string(&value)?;
        let _ = FusionConfig::from_toml_with_context(&expanded, &file_ctx)?;
        let mut origins = raw.origins.clone();
        for (path, source_set) in expanded_with_sources.sources {
            origins.insert(path, PathBuf::from(render_source_label(&source_set)));
        }
        Ok(RawFusionConfigTree { value, origins })
    }

    pub fn load_effective_vars(&self) -> anyhow::Result<Vec<ResolvedConfigVar>> {
        let raw = self.load_raw()?;
        let file_ctx = self.ctx.for_file(self.base_path);
        let mut effective_vars = resolve_value_vars_with_sources(raw.value(), &file_ctx, |path| {
            raw.origin_for(path).map(Path::to_path_buf)
        })?;

        for source in collect_active_external_sources(raw.value(), &effective_vars, &file_ctx)? {
            let ident = match source {
                SourceAtom::Explicit(ident)
                | SourceAtom::Builtin(ident)
                | SourceAtom::Env(ident) => ident,
                SourceAtom::Default(_) | SourceAtom::File(_) => continue,
            };
            if effective_vars.contains_key(&ident) {
                continue;
            }
            if let Some(value) = external_value_with_source(&ident, &file_ctx) {
                effective_vars.insert(ident, value);
            }
        }

        let mut entries = Vec::with_capacity(effective_vars.len());
        for (key, value) in effective_vars {
            entries.push(ResolvedConfigVar {
                key,
                value: value.value,
                source: render_source_label(&value.sources),
            });
        }
        entries.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(entries)
    }
}

fn canonicalize_existing_dir(path: &Path) -> anyhow::Result<PathBuf> {
    path.canonicalize()
        .map_err(|e| anyhow::anyhow!("failed to resolve {}: {e}", path.display()))
}

fn read_toml_file(path: &Path) -> anyhow::Result<RawFusionConfigTree> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let value = parse_toml_table(&content, path)?;
    Ok(RawFusionConfigTree::new(value, path))
}

fn parse_toml_table(content: &str, path: &Path) -> anyhow::Result<TomlValue> {
    let value: TomlValue = toml::from_str(content)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    if !value.is_table() {
        anyhow::bail!("fusion config {} must be a TOML table", path.display());
    }
    Ok(value)
}

fn merge_value_with_origins(
    base: &mut TomlValue,
    base_origins: &mut HashMap<String, PathBuf>,
    overlay: TomlValue,
    overlay_origins: &HashMap<String, PathBuf>,
    path: Option<&str>,
) {
    match (base, overlay) {
        (TomlValue::Table(base_table), TomlValue::Table(overlay_table)) => {
            for (key, overlay_value) in overlay_table {
                let child_path = join_path(path, &key);
                match base_table.get_mut(&key) {
                    Some(base_value) => merge_value_with_origins(
                        base_value,
                        base_origins,
                        overlay_value,
                        overlay_origins,
                        Some(&child_path),
                    ),
                    None => {
                        base_table.insert(key, overlay_value);
                        copy_origin_subtree(base_origins, overlay_origins, Some(&child_path));
                    }
                }
            }
        }
        (base_slot, overlay_value) => {
            clear_origin_subtree(base_origins, path);
            *base_slot = overlay_value;
            copy_origin_subtree(base_origins, overlay_origins, path);
        }
    }
}

fn rebase_overlay_paths(value: &mut TomlValue, overlay_dir: &Path, target_base_dir: &Path) {
    rebase_top_level_path(value, "sinks", overlay_dir, target_base_dir);
    rebase_top_level_path(value, "work_root", overlay_dir, target_base_dir);
    rebase_nested_path(value, &["runtime", "schemas"], overlay_dir, target_base_dir);
    rebase_nested_path(value, &["runtime", "rules"], overlay_dir, target_base_dir);
    rebase_nested_path(value, &["logging", "file"], overlay_dir, target_base_dir);
    rebase_source_paths(value, overlay_dir, target_base_dir);
}

fn rebase_top_level_path(
    value: &mut TomlValue,
    key: &str,
    overlay_dir: &Path,
    target_base_dir: &Path,
) {
    let Some(table) = value.as_table_mut() else {
        return;
    };
    let Some(entry) = table.get_mut(key) else {
        return;
    };
    rebase_path_string_value(entry, overlay_dir, target_base_dir);
}

fn rebase_nested_path(
    value: &mut TomlValue,
    path: &[&str],
    overlay_dir: &Path,
    target_base_dir: &Path,
) {
    if path.is_empty() {
        return;
    }
    let mut current = value;
    for key in &path[..path.len() - 1] {
        let Some(next) = current.get_mut(*key) else {
            return;
        };
        current = next;
    }
    let Some(last) = path.last() else {
        return;
    };
    let Some(entry) = current.get_mut(*last) else {
        return;
    };
    rebase_path_string_value(entry, overlay_dir, target_base_dir);
}

fn rebase_source_paths(value: &mut TomlValue, overlay_dir: &Path, target_base_dir: &Path) {
    let Some(sources) = value.get_mut("sources").and_then(TomlValue::as_array_mut) else {
        return;
    };
    for source in sources {
        let Some(path_value) = source.get_mut("path") else {
            continue;
        };
        rebase_path_string_value(path_value, overlay_dir, target_base_dir);
    }
}

fn rebase_path_string_value(value: &mut TomlValue, overlay_dir: &Path, target_base_dir: &Path) {
    let Some(raw) = value.as_str() else {
        return;
    };
    let Some(rebased) = rebase_relative_path_string(raw, overlay_dir, target_base_dir) else {
        return;
    };
    *value = TomlValue::String(rebased);
}

fn rebase_relative_path_string(
    raw: &str,
    overlay_dir: &Path,
    target_base_dir: &Path,
) -> Option<String> {
    if raw.contains('$') || raw.is_empty() {
        return None;
    }
    let candidate = Path::new(raw);
    if candidate.is_absolute() {
        return None;
    }

    let overlay_target = normalize_path(overlay_dir.join(candidate));
    let rebased = diff_paths(&overlay_target, target_base_dir)
        .unwrap_or(overlay_target)
        .to_string_lossy()
        .to_string();
    Some(rebased)
}

fn normalize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut prefix: Option<OsString> = None;
    let mut has_root = false;
    let mut parts: Vec<OsString> = Vec::new();

    for component in path.components() {
        match component {
            Component::Prefix(p) => prefix = Some(p.as_os_str().to_os_string()),
            Component::RootDir => has_root = true,
            Component::CurDir => {}
            Component::ParentDir => {
                if let Some(last) = parts.last()
                    && last != ".."
                {
                    parts.pop();
                    continue;
                }
                if !has_root {
                    parts.push(OsString::from(".."));
                }
            }
            Component::Normal(part) => parts.push(part.to_os_string()),
        }
    }

    let mut out = PathBuf::new();
    if let Some(prefix) = prefix {
        out.push(prefix);
    }
    if has_root {
        out.push(Path::new("/"));
    }
    for part in parts {
        out.push(part);
    }
    if out.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        out
    }
}

fn diff_paths(path: &Path, base: &Path) -> Option<PathBuf> {
    use std::path::Component;

    let path_components: Vec<Component<'_>> = path.components().collect();
    let base_components: Vec<Component<'_>> = base.components().collect();

    let same_prefix_kind = matches!(
        (path_components.first(), base_components.first()),
        (Some(Component::Prefix(a)), Some(Component::Prefix(b))) if a == b
    ) || !matches!(
        (path_components.first(), base_components.first()),
        (Some(Component::Prefix(_)), _) | (_, Some(Component::Prefix(_)))
    );
    if !same_prefix_kind {
        return None;
    }

    let mut common = 0usize;
    while common < path_components.len()
        && common < base_components.len()
        && path_components[common] == base_components[common]
    {
        common += 1;
    }

    let mut result = PathBuf::new();
    for component in &base_components[common..] {
        if matches!(component, Component::Normal(_)) {
            result.push("..");
        }
    }
    for component in &path_components[common..] {
        result.push(component.as_os_str());
    }

    if result.as_os_str().is_empty() {
        Some(PathBuf::from("."))
    } else {
        Some(result)
    }
}

fn record_origins(
    value: &TomlValue,
    source_path: &Path,
    path: Option<&str>,
    origins: &mut HashMap<String, PathBuf>,
) {
    if let Some(path) = path {
        origins.insert(path.to_string(), source_path.to_path_buf());
    }
    match value {
        TomlValue::Table(table) => {
            for (key, child) in table {
                let child_path = join_path(path, key);
                record_origins(child, source_path, Some(&child_path), origins);
            }
        }
        TomlValue::Array(items) => {
            for (idx, child) in items.iter().enumerate() {
                let child_path = join_indexed_path(path, idx);
                record_origins(child, source_path, Some(&child_path), origins);
            }
        }
        _ => {}
    }
}

fn join_path(parent: Option<&str>, key: &str) -> String {
    match parent {
        Some(parent) if !parent.is_empty() => format!("{parent}.{key}"),
        _ => key.to_string(),
    }
}

fn join_indexed_path(parent: Option<&str>, idx: usize) -> String {
    match parent {
        Some(parent) if !parent.is_empty() => format!("{parent}[{idx}]"),
        _ => format!("[{idx}]"),
    }
}

fn clear_origin_subtree(origins: &mut HashMap<String, PathBuf>, path: Option<&str>) {
    let Some(path) = path else {
        origins.clear();
        return;
    };
    origins.retain(|key, _| !path_matches_or_descends(key, path));
}

fn copy_origin_subtree(
    dst: &mut HashMap<String, PathBuf>,
    src: &HashMap<String, PathBuf>,
    path: Option<&str>,
) {
    match path {
        Some(path) => {
            for (key, origin) in src {
                if path_matches_or_descends(key, path) {
                    dst.insert(key.clone(), origin.clone());
                }
            }
        }
        None => {
            for (key, origin) in src {
                dst.insert(key.clone(), origin.clone());
            }
        }
    }
}

fn path_matches_or_descends(key: &str, path: &str) -> bool {
    key == path
        || key
            .strip_prefix(path)
            .is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
}

fn diff_value(
    old_value: Option<&TomlValue>,
    new_value: Option<&TomlValue>,
    path: &str,
    old_tree: &RawFusionConfigTree,
    new_tree: &RawFusionConfigTree,
    changes: &mut Vec<RawFusionConfigChange>,
) {
    match (old_value, new_value) {
        (Some(TomlValue::Table(old_table)), Some(TomlValue::Table(new_table))) => {
            let mut keys: Vec<&str> = old_table
                .keys()
                .map(String::as_str)
                .chain(new_table.keys().map(String::as_str))
                .collect();
            keys.sort_unstable();
            keys.dedup();
            for key in keys {
                let child_path = if path.is_empty() {
                    key.to_string()
                } else {
                    format!("{path}.{key}")
                };
                diff_value(
                    old_table.get(key),
                    new_table.get(key),
                    &child_path,
                    old_tree,
                    new_tree,
                    changes,
                );
            }
        }
        (Some(TomlValue::Array(old_items)), Some(TomlValue::Array(new_items))) => {
            if old_items != new_items {
                changes.push(RawFusionConfigChange {
                    path: path.to_string(),
                    old_value: old_value.cloned(),
                    new_value: new_value.cloned(),
                    old_origin: (!path.is_empty())
                        .then(|| old_tree.origin_for(path).map(Path::to_path_buf))
                        .flatten(),
                    new_origin: (!path.is_empty())
                        .then(|| new_tree.origin_for(path).map(Path::to_path_buf))
                        .flatten(),
                });
            }
        }
        (Some(old_leaf), Some(new_leaf)) => {
            if old_leaf != new_leaf {
                changes.push(RawFusionConfigChange {
                    path: path.to_string(),
                    old_value: Some(old_leaf.clone()),
                    new_value: Some(new_leaf.clone()),
                    old_origin: (!path.is_empty())
                        .then(|| old_tree.origin_for(path).map(Path::to_path_buf))
                        .flatten(),
                    new_origin: (!path.is_empty())
                        .then(|| new_tree.origin_for(path).map(Path::to_path_buf))
                        .flatten(),
                });
            }
        }
        (None, Some(_)) | (Some(_), None) => {
            changes.push(RawFusionConfigChange {
                path: path.to_string(),
                old_value: old_value.cloned(),
                new_value: new_value.cloned(),
                old_origin: (!path.is_empty())
                    .then(|| old_tree.origin_for(path).map(Path::to_path_buf))
                    .flatten(),
                new_origin: (!path.is_empty())
                    .then(|| new_tree.origin_for(path).map(Path::to_path_buf))
                    .flatten(),
            });
        }
        (None, None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "wf-config-loader-{}-{}-{}",
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
    fn raw_loader_tracks_field_origins_after_overlay_merge() {
        let root = make_temp_dir("origin-merge");
        let base_path = root.join("conf/base.toml");
        let overlay_path = root.join("env/dev/overlay.toml");
        write_file(
            &base_path,
            r#"
mode = "daemon"
sinks = "sinks"

[[sources]]
type = "tcp"
name = "ingress"
listen = "tcp://127.0.0.1:9800"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/base/*.wfs"
rules = "rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
"#,
        );
        write_file(
            &overlay_path,
            r#"
mode = "batch"

[[sources]]
type = "file"
path = "../data/seed.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
rules = "../rules/dev/*.wfl"

[window.overlay_events]
mode = "replicated"
max_window_bytes = "64MB"
over_cap = "48h"
"#,
        );

        let raw = FusionConfigLoader::new(
            &base_path,
            std::slice::from_ref(&overlay_path),
            &ConfigVarContext::new(),
        )
        .load_raw()
        .expect("load raw");

        assert_eq!(raw.origin_for("mode"), Some(overlay_path.as_path()));
        assert_eq!(raw.origin_for("runtime.schemas"), Some(base_path.as_path()));
        assert_eq!(
            raw.origin_for("runtime.rules"),
            Some(overlay_path.as_path())
        );
        assert_eq!(
            raw.origin_for("window.base_events.mode"),
            Some(base_path.as_path())
        );
        assert_eq!(
            raw.origin_for("window.overlay_events.mode"),
            Some(overlay_path.as_path())
        );
        assert_eq!(
            raw.origin_for("sources[0].path"),
            Some(overlay_path.as_path())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_loader_tracks_rebased_overlay_path_values() {
        let root = make_temp_dir("origin-rebase");
        let base_path = root.join("conf/base.toml");
        let overlay_path = root.join("env/dev/overlay.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "sinks"

[[sources]]
type = "file"
path = "data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/base/*.wfs"
rules = "rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
"#,
        );
        write_file(
            &overlay_path,
            r#"
sinks = "../sinks/dev"

[runtime]
rules = "../rules/dev/*.wfl"
"#,
        );

        let raw = FusionConfigLoader::new(
            &base_path,
            std::slice::from_ref(&overlay_path),
            &ConfigVarContext::new(),
        )
        .load_raw()
        .expect("load raw");
        let sinks = raw
            .value()
            .get("sinks")
            .and_then(TomlValue::as_str)
            .expect("sinks string");
        let rules = raw
            .value()
            .get("runtime")
            .and_then(|v| v.get("rules"))
            .and_then(TomlValue::as_str)
            .expect("rules string");

        assert_eq!(sinks, "../env/sinks/dev");
        assert_eq!(rules, "../env/rules/dev/*.wfl");
        assert_eq!(raw.origin_for("sinks"), Some(overlay_path.as_path()));
        assert_eq!(
            raw.origin_for("runtime.rules"),
            Some(overlay_path.as_path())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn raw_loader_diff_reports_changed_values_and_origins() {
        let root = make_temp_dir("origin-diff");
        let base_path = root.join("conf/base.toml");
        let old_overlay = root.join("env/dev/old.toml");
        let new_overlay = root.join("env/dev/new.toml");
        write_file(
            &base_path,
            r#"
mode = "daemon"
sinks = "sinks"

[[sources]]
type = "tcp"
name = "ingress"
listen = "tcp://127.0.0.1:9800"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/base/*.wfs"
rules = "rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
"#,
        );
        write_file(
            &old_overlay,
            r#"
mode = "batch"

[runtime]
rules = "../rules/v1/*.wfl"
"#,
        );
        write_file(
            &new_overlay,
            r#"
mode = "batch"

[runtime]
rules = "../rules/v2/*.wfl"

[vars]
CASE_PATH = "/tmp/case-a"
"#,
        );

        let old_raw = FusionConfigLoader::new(
            &base_path,
            std::slice::from_ref(&old_overlay),
            &ConfigVarContext::new(),
        )
        .load_raw()
        .expect("load old raw");
        let new_raw = FusionConfigLoader::new(
            &base_path,
            std::slice::from_ref(&new_overlay),
            &ConfigVarContext::new(),
        )
        .load_raw()
        .expect("load new raw");

        let changes = old_raw.diff(&new_raw);
        assert_eq!(changes.len(), 2);

        let rules_change = changes
            .iter()
            .find(|c| c.path == "runtime.rules")
            .expect("runtime.rules change");
        assert_eq!(
            rules_change.old_value,
            Some(TomlValue::String("../env/rules/v1/*.wfl".to_string()))
        );
        assert_eq!(
            rules_change.new_value,
            Some(TomlValue::String("../env/rules/v2/*.wfl".to_string()))
        );
        assert_eq!(
            rules_change.old_origin.as_deref(),
            Some(old_overlay.as_path())
        );
        assert_eq!(
            rules_change.new_origin.as_deref(),
            Some(new_overlay.as_path())
        );

        let vars_change = changes
            .iter()
            .find(|c| c.path == "vars")
            .expect("vars change");
        assert_eq!(vars_change.old_value, None);
        let mut expected_vars = toml::map::Map::new();
        expected_vars.insert(
            "CASE_PATH".to_string(),
            TomlValue::String("/tmp/case-a".to_string()),
        );
        assert_eq!(vars_change.new_value, Some(TomlValue::Table(expected_vars)));
        assert_eq!(vars_change.old_origin, None);
        assert_eq!(
            vars_change.new_origin.as_deref(),
            Some(new_overlay.as_path())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_expanded_toml_renders_overlay_and_vars_in_final_output() {
        let root = make_temp_dir("expanded-render");
        let base_path = root.join("conf/base.toml");
        let overlay_path = root.join("env/dev/overlay.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"

[[sources]]
type = "file"
path = "${CASE_PATH}/data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "${CASE_PATH}/schemas/base/*.wfs"
rules = "${CASE_PATH}/rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
CASE_PATH = "/tmp/base"
"#,
        );
        write_file(
            &overlay_path,
            r#"
[runtime]
rules = "../rules/dev/*.wfl"

[vars]
CASE_PATH = "/tmp/overlay"
"#,
        );

        let expanded = FusionConfigLoader::new(
            &base_path,
            std::slice::from_ref(&overlay_path),
            &ConfigVarContext::new(),
        )
        .load_expanded_toml()
        .expect("load expanded toml");

        assert!(expanded.contains("sinks = \"/tmp/overlay/sinks\""));
        assert!(expanded.contains("path = \"/tmp/overlay/data/base.ndjson\""));
        assert!(expanded.contains("schemas = \"/tmp/overlay/schemas/base/*.wfs\""));
        assert!(expanded.contains("rules = \"../env/rules/dev/*.wfl\""));
        assert!(expanded.contains("CASE_PATH = \"/tmp/overlay\""));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn origin_entries_are_sorted_by_path() {
        let root = make_temp_dir("origin-entries");
        let base_path = root.join("conf/base.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "sinks"

[[sources]]
type = "file"
path = "data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/base/*.wfs"
rules = "rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
"#,
        );

        let raw = FusionConfigLoader::new(&base_path, &[], &ConfigVarContext::new())
            .load_raw()
            .expect("load raw");
        let entries = raw.origin_entries();

        assert!(!entries.is_empty());
        let paths: Vec<&str> = entries.iter().map(|(path, _)| path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_expanded_raw_keeps_origins_and_expands_values() {
        let root = make_temp_dir("expanded-raw");
        let base_path = root.join("conf/base.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"

[[sources]]
type = "file"
path = "${CASE_PATH}/data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "${CASE_PATH}/schemas/base/*.wfs"
rules = "${CASE_PATH}/rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
CASE_PATH = "/tmp/base"
"#,
        );

        let expanded = FusionConfigLoader::new(&base_path, &[], &ConfigVarContext::new())
            .load_expanded_raw()
            .expect("load expanded raw");

        let sinks = expanded
            .value()
            .get("sinks")
            .and_then(TomlValue::as_str)
            .expect("sinks string");
        assert_eq!(sinks, "/tmp/base/sinks");
        assert_eq!(expanded.origin_for("sinks"), Some(base_path.as_path()));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_expanded_raw_aggregates_array_field_sources() {
        let root = make_temp_dir("expanded-raw-array");
        let base_path = root.join("conf/base.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"

[[sources]]
type = "file"
path = "${CASE_PATH}/data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "${CASE_PATH}/schemas/base/*.wfs"
rules = "${CASE_PATH}/rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
"#,
        );

        let mut explicit = HashMap::new();
        explicit.insert("CASE_PATH".to_string(), "/tmp/from-cli".to_string());
        let expanded = FusionConfigLoader::new(
            &base_path,
            &[],
            &ConfigVarContext::from_explicit_vars(explicit),
        )
        .load_expanded_raw()
        .expect("load expanded raw");

        let origin = expanded
            .origin_for("sources")
            .map(|path| path.to_string_lossy().to_string())
            .expect("sources origin");
        assert!(origin.starts_with("<mixed:"));
        assert!(origin.contains(&format!("file:{}", base_path.display())));
        assert!(origin.contains("cli:CASE_PATH"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_effective_vars_reports_final_value_sources() {
        let root = make_temp_dir("effective-vars");
        let base_path = root.join("conf/base.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"
work_root = "$HOME"

[[sources]]
type = "file"
path = "data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/base/*.wfs"
rules = "rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
CASE_PATH = "${HOME}/case"
MIXED = "${CASE_PATH}/tail"
"#,
        );

        let mut explicit = HashMap::new();
        explicit.insert("CASE_PATH".to_string(), "/tmp/from-cli".to_string());
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).expect("failed to create workspace");
        let ctx = ConfigVarContext::from_explicit_vars(explicit).with_work_dir(Some(workspace));
        let vars = FusionConfigLoader::new(&base_path, &[], &ctx)
            .load_effective_vars()
            .expect("load effective vars");
        let home = std::env::var("HOME").expect("HOME env var");

        assert!(vars.iter().any(|entry| {
            entry.key == "CASE_PATH"
                && entry.value == "/tmp/from-cli"
                && entry.source == "<cli:CASE_PATH>"
        }));
        assert!(
            vars.iter().any(|entry| {
                entry.key == "CONFIG_DIR" && entry.source == "<builtin:CONFIG_DIR>"
            })
        );
        assert!(
            vars.iter()
                .any(|entry| { entry.key == "WORK_DIR" && entry.source == "<builtin:WORK_DIR>" })
        );
        assert!(vars.iter().any(|entry| {
            entry.key == "HOME" && entry.value == home && entry.source == "<env:HOME>"
        }));
        assert!(vars.iter().any(|entry| {
            entry.key == "MIXED"
                && entry.source == format!("<mixed:file:{},cli:CASE_PATH>", base_path.display())
        }));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_effective_vars_does_not_leak_env_from_overridden_file_var() {
        let root = make_temp_dir("effective-vars-overridden");
        let base_path = root.join("conf/base.toml");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"

[[sources]]
type = "file"
path = "${CASE_PATH}/data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/base/*.wfs"
rules = "rules/base/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.base_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
CASE_PATH = "${HOME}/case"
"#,
        );

        let mut explicit = HashMap::new();
        explicit.insert("CASE_PATH".to_string(), "/tmp/from-cli".to_string());
        let vars = FusionConfigLoader::new(
            &base_path,
            &[],
            &ConfigVarContext::from_explicit_vars(explicit),
        )
        .load_effective_vars()
        .expect("load effective vars");

        assert!(vars.iter().any(|entry| {
            entry.key == "CASE_PATH"
                && entry.value == "/tmp/from-cli"
                && entry.source == "<cli:CASE_PATH>"
        }));
        assert!(!vars.iter().any(|entry| entry.key == "HOME"));

        let _ = std::fs::remove_dir_all(root);
    }
}
