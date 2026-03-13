use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wp_connector_api::ConnectorDef;

use super::build::{build_fixed_group, build_flex_group};
use super::connector::load_connector_defs;
use super::defaults::{DefaultsBody, load_defaults};
use super::group::{FixedGroup, FlexGroup};
use super::route::RouteFile;

// ---------------------------------------------------------------------------
// SinkConfigBundle — aggregated result of loading all sink config files
// ---------------------------------------------------------------------------

/// The complete sink configuration loaded from a `sinks/` directory.
#[derive(Debug)]
pub struct SinkConfigBundle {
    /// Connector definitions loaded from `sink.d/`.
    pub connectors: BTreeMap<String, ConnectorDef>,
    /// Global defaults from `defaults.toml`.
    pub defaults: DefaultsBody,
    /// Business routing groups loaded from `business.d/`.
    pub business: Vec<FlexGroup>,
    /// Infrastructure default group from `infra.d/default.toml`.
    pub infra_default: Option<FixedGroup>,
    /// Infrastructure error group from `infra.d/error.toml`.
    pub infra_error: Option<FixedGroup>,
}

// ---------------------------------------------------------------------------
// Directory loading
// ---------------------------------------------------------------------------

/// Load the complete sink configuration from a `sinks/` root directory.
///
/// Expected directory layout:
/// ```text
/// sinks/
/// ├── sink.d/               # connector definitions (*.toml)
/// ├── business.d/           # business routing groups (*.toml)
/// ├── infra.d/              # infrastructure groups (*.toml)
/// │   ├── default.toml
/// │   └── error.toml
/// └── defaults.toml         # global defaults
/// ```
///
/// Connector definitions are loaded from the nearest ancestor containing
/// `connectors/sink.d`.
pub fn load_sink_config(sink_root: &Path) -> anyhow::Result<SinkConfigBundle> {
    if !sink_root.is_dir() {
        anyhow::bail!(
            "sink config directory does not exist: {}",
            sink_root.display()
        );
    }

    // 1. Load connectors from sink.d/
    let connectors = if let Some(connector_dir) = find_connector_dir(sink_root) {
        load_connector_defs(&connector_dir)?
    } else {
        BTreeMap::new()
    };

    // 2. Load defaults
    let defaults = load_defaults(sink_root)?;

    // 3. Load business groups from business.d/
    let business = load_business_groups(&sink_root.join("business.d"), &connectors, &defaults)?;

    // 4. Load infra groups from infra.d/
    let infra_default = load_infra_group(
        &sink_root.join("infra.d").join("default.toml"),
        &connectors,
        &defaults,
    )?;
    let infra_error = load_infra_group(
        &sink_root.join("infra.d").join("error.toml"),
        &connectors,
        &defaults,
    )?;

    Ok(SinkConfigBundle {
        connectors,
        defaults,
        business,
        infra_default,
        infra_error,
    })
}

fn find_connector_dir(sink_root: &Path) -> Option<PathBuf> {
    let base = if sink_root.is_absolute() {
        sink_root.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(sink_root)
    };
    let mut current = if base.is_dir() {
        base
    } else {
        base.parent()?.to_path_buf()
    };

    for _ in 0..32 {
        let candidate = current.join("connectors").join("sink.d");
        if candidate.is_dir() {
            return Some(candidate);
        }
        if !current.pop() {
            break;
        }
    }

    None
}

/// Load all business routing groups from `*.toml` files in a directory.
fn load_business_groups(
    dir: &Path,
    connectors: &BTreeMap<String, ConnectorDef>,
    defaults: &DefaultsBody,
) -> anyhow::Result<Vec<FlexGroup>> {
    let mut groups = Vec::new();

    if !dir.is_dir() {
        return Ok(groups);
    }

    let pattern = dir.join("*.toml");
    let pattern_str = pattern.to_string_lossy();

    let mut entries: Vec<_> = glob::glob(&pattern_str)?.filter_map(|e| e.ok()).collect();
    entries.sort();

    for path in entries {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        let file: RouteFile = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;

        let group = build_flex_group(&file.sink_group, connectors, defaults)
            .map_err(|e| anyhow::anyhow!("error in {}: {e}", path.display()))?;
        groups.push(group);
    }

    Ok(groups)
}

/// Load a single infra group from a TOML file (returns None if file doesn't exist).
fn load_infra_group(
    path: &Path,
    connectors: &BTreeMap<String, ConnectorDef>,
    defaults: &DefaultsBody,
) -> anyhow::Result<Option<FixedGroup>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
    let file: RouteFile = toml::from_str(&content)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;

    let group = build_fixed_group(&file.sink_group, connectors, defaults)
        .map_err(|e| anyhow::anyhow!("error in {}: {e}", path.display()))?;
    Ok(Some(group))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "wf-config-{}-{}-{}",
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

    fn sample_connector_toml() -> &'static str {
        r#"
[[connectors]]
id = "file_json"
type = "file"
allow_override = ["file"]

[connectors.params]
fmt = "json"
file = "default.jsonl"
"#
    }

    fn sample_business_toml() -> &'static str {
        r#"
[sink_group]
name = "catch_all"
windows = ["*"]

[[sink_group.sinks]]
connect = "file_json"

[sink_group.sinks.params]
file = "all.jsonl"
"#
    }

    #[test]
    fn loads_connectors_from_ancestor_connectors_dir() {
        let root = make_temp_dir("ancestor-connectors");
        let sink_root = root.join("examples/sinks");

        write_file(
            &root.join("examples/connectors/sink.d/file_json.toml"),
            sample_connector_toml(),
        );
        write_file(&sink_root.join("business.d/catch_all.toml"), sample_business_toml());

        let bundle = load_sink_config(&sink_root).expect("load sink config");
        assert!(bundle.connectors.contains_key("file_json"));
        assert_eq!(bundle.business.len(), 1);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn returns_none_when_no_ancestor_connectors_dir_exists() {
        let root = make_temp_dir("no-connectors");
        let sink_root = root.join("examples/sinks");

        std::fs::create_dir_all(&sink_root).expect("failed to create sink root");
        assert_eq!(find_connector_dir(&sink_root), None);

        let _ = std::fs::remove_dir_all(root);
    }
}
