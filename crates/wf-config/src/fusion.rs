use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use orion_error::conversion::{ConvErr, SourceRawErr};
use serde::{Deserialize, Serialize};

use crate::error::{ConfigReason, ConfigResult};
use crate::loader::FusionConfigLoader;
use crate::logging::LoggingConfig;
use crate::metrics::MetricsConfig;
use crate::runtime::RuntimeConfig;
use crate::source::SourceConfig;
use crate::validate;
use crate::vars::inject_loader_scoped_vars;
use crate::window::{WindowConfig, WindowDefaults, WindowOverride};
use toml::Value as TomlValue;
use wf_vars::{ConfigVarContext, expand_value};

#[derive(::moju_derive::MoJu, Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
#[moju(kind = "state", domain = "Config", module = "Config.ConfigLoader")]
pub enum FusionMode {
    #[default]
    Daemon,
    Batch,
}

// ---------------------------------------------------------------------------
// Raw TOML structure (intermediate representation)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct FusionConfigRaw {
    #[serde(default)]
    mode: FusionMode,
    runtime: RuntimeConfig,
    window_defaults: WindowDefaults,
    #[serde(default)]
    window: HashMap<String, WindowOverride>,
    /// Path to the sinks/ directory for connector-based sink routing.
    sinks: String,
    /// Optional working root for sink file-path resolution.
    #[serde(default)]
    work_root: Option<String>,
    #[serde(default)]
    logging: LoggingConfig,
    #[serde(default)]
    metrics: MetricsConfig,
    /// User-defined variables for WFL `$VAR` / `${VAR:default}` preprocessing.
    #[serde(default)]
    vars: HashMap<String, String>,
    /// Data input sources (`tcp` / `file`).
    #[serde(default)]
    sources: Vec<SourceConfig>,
}

// ---------------------------------------------------------------------------
// FusionConfig (resolved, validated)
// ---------------------------------------------------------------------------

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq)]
#[moju(kind = "struct", domain = "Config", module = "Config.ConfigLoader")]
pub struct FusionConfig {
    pub mode: FusionMode,
    pub runtime: RuntimeConfig,
    pub window_defaults: WindowDefaults,
    pub windows: Vec<WindowConfig>,
    /// Path to the sinks/ directory for connector-based sink routing.
    pub sinks: String,
    /// Optional working root for sink file-path resolution.
    pub work_root: Option<String>,
    pub logging: LoggingConfig,
    pub metrics: MetricsConfig,
    /// User-defined variables for WFL `$VAR` / `${VAR:default}` preprocessing.
    pub vars: HashMap<String, String>,
    /// Resolved input source list.
    pub sources: Vec<SourceConfig>,
}

impl FusionConfig {
    /// Read and parse a `wfusion.toml` file.
    pub fn load(path: impl AsRef<Path>) -> ConfigResult<Self> {
        Self::load_with_context(path, &ConfigVarContext::new(), None)
    }

    /// Read and parse a base `wfusion.toml` file plus overlay files.
    pub fn load_with_overlays(
        path: impl AsRef<Path>,
        overlay_paths: &[PathBuf],
        ctx: &ConfigVarContext,
        work_dir: Option<&Path>,
    ) -> ConfigResult<Self> {
        FusionConfigLoader::new(path.as_ref(), overlay_paths, ctx, work_dir).load()
    }

    /// Read and parse a `wfusion.toml` file with an explicit variable context.
    pub fn load_with_context(
        path: impl AsRef<Path>,
        ctx: &ConfigVarContext,
        work_dir: Option<&Path>,
    ) -> ConfigResult<Self> {
        Self::load_with_overlays(path, &[], ctx, work_dir)
    }

    pub(crate) fn from_toml_with_context(
        toml_str: &str,
        ctx: &ConfigVarContext,
    ) -> ConfigResult<Self> {
        let value: TomlValue =
            toml::from_str(toml_str).source_raw_err(ConfigReason::Parse, "parse fusion TOML")?;
        Self::from_value_with_context(&value, ctx, None, None)
    }

    pub(crate) fn from_value_with_context(
        value: &TomlValue,
        ctx: &ConfigVarContext,
        source_path: Option<&Path>,
        work_dir: Option<&Path>,
    ) -> ConfigResult<Self> {
        let scoped = match source_path {
            Some(path) => {
                inject_loader_scoped_vars(value, path, work_dir.or_else(|| path.parent()))
            }
            None => value.clone(),
        };
        let expanded = expand_value(&scoped, ctx).conv_err()?;
        let expanded_toml = toml::to_string(&expanded)
            .source_raw_err(ConfigReason::Parse, "serialize expanded fusion TOML")?;
        let mut raw: FusionConfigRaw = toml::from_str(&expanded_toml)
            .source_raw_err(ConfigReason::Parse, "parse expanded fusion TOML")?;
        raw.vars = ctx.materialize_vars(&raw.vars);

        // Resolve window overrides against defaults.
        let mut windows = Vec::with_capacity(raw.window.len());
        for (name, ovr) in raw.window {
            let wc = ovr.resolve(name, &raw.window_defaults)?;
            windows.push(wc);
        }
        // Sort by name for deterministic ordering.
        windows.sort_by(|a, b| a.name.cmp(&b.name));

        let config = FusionConfig {
            mode: raw.mode,
            runtime: raw.runtime,
            window_defaults: raw.window_defaults,
            windows,
            sinks: raw.sinks,
            work_root: raw.work_root,
            logging: raw.logging,
            metrics: raw.metrics,
            vars: raw.vars,
            sources: raw.sources,
        };

        validate::validate(&config)?;

        Ok(config)
    }
}

impl FromStr for FusionConfig {
    type Err = crate::ConfigError;

    /// Parse a TOML string into a resolved, validated [`FusionConfig`].
    fn from_str(toml_str: &str) -> ConfigResult<Self> {
        Self::from_toml_with_context(toml_str, &ConfigVarContext::new())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{FileInputFormat, SourceConfig};
    use crate::types::{ByteSize, DistMode, EvictPolicy, HumanDuration, LatePolicy};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn make_temp_dir(name: &str) -> PathBuf {
        let unique = format!(
            "wf-config-fusion-{}-{}-{}",
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

    const FULL_TOML: &str = r#"
mode = "daemon"
sinks = "sinks"

[[sources]]
type = "tcp"
name = "ingress"
listen = "tcp://127.0.0.1:9800"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"
rules   = "rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.auth_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[window.fw_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
watermark = "10s"
allowed_lateness = "30s"
late_policy = "drop"

[window.ip_blocklist]
mode = "replicated"
max_window_bytes = "64MB"
over_cap = "48h"
"#;

    #[test]
    fn load_full_toml() {
        let cfg: FusionConfig = FULL_TOML.parse().unwrap();

        // mode
        assert_eq!(cfg.mode, FusionMode::Daemon);

        // runtime
        assert_eq!(cfg.runtime.executor_parallelism, 2);
        assert_eq!(
            cfg.runtime.rule_exec_timeout.as_duration(),
            Duration::from_secs(30),
        );
        assert_eq!(cfg.runtime.schemas, "schemas/*.wfs");
        assert_eq!(cfg.runtime.rules, "rules/*.wfl");

        // window_defaults
        assert_eq!(
            cfg.window_defaults.evict_interval,
            "30s".parse::<HumanDuration>().unwrap(),
        );
        assert_eq!(cfg.window_defaults.evict_policy, EvictPolicy::TimeFirst);
        assert_eq!(cfg.window_defaults.late_policy, LatePolicy::Drop);

        // windows (sorted by name)
        assert_eq!(cfg.windows.len(), 3);
        assert_eq!(cfg.windows[0].name, "auth_events");
        assert_eq!(cfg.windows[0].mode, DistMode::Local);
        assert_eq!(
            cfg.windows[0].over_cap.as_duration(),
            Duration::from_secs(30 * 60),
        );
        // auth_events inherits watermark from defaults
        assert_eq!(
            cfg.windows[0].watermark,
            "5s".parse::<HumanDuration>().unwrap(),
        );

        assert_eq!(cfg.windows[1].name, "fw_events");
        assert_eq!(
            cfg.windows[1].watermark,
            "10s".parse::<HumanDuration>().unwrap(),
        );
        assert_eq!(
            cfg.windows[1].allowed_lateness,
            "30s".parse::<HumanDuration>().unwrap(),
        );

        assert_eq!(cfg.windows[2].name, "ip_blocklist");
        assert_eq!(cfg.windows[2].mode, DistMode::Replicated);
        assert_eq!(
            cfg.windows[2].max_window_bytes,
            "64MB".parse::<ByteSize>().unwrap(),
        );

        // sinks
        assert_eq!(cfg.sinks, "sinks");
        assert!(!cfg.metrics.enabled);
        assert_eq!(
            cfg.metrics.report_interval.as_duration(),
            Duration::from_secs(2)
        );
        assert_eq!(cfg.metrics.prometheus_listen, "127.0.0.1:9901");
        assert_eq!(cfg.sources.len(), 1);
        match &cfg.sources[0] {
            SourceConfig::Tcp(tcp) => {
                assert_eq!(tcp.name.as_deref(), Some("ingress"));
                assert_eq!(tcp.listen, "tcp://127.0.0.1:9800");
                assert!(tcp.enabled);
            }
            SourceConfig::File(_) => panic!("unexpected file source"),
        }
    }

    #[test]
    fn reject_invalid_tcp_source_listen() {
        let toml = FULL_TOML.replace("tcp://127.0.0.1:9800", "http://bad");
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn reject_zero_parallelism() {
        let toml = FULL_TOML.replace("executor_parallelism = 2", "executor_parallelism = 0");
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn reject_partitioned_no_key() {
        let toml = FULL_TOML.replace(
            "[window.auth_events]\nmode = \"local\"",
            "[window.auth_events]\nmode = \"partitioned\"",
        );
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn reject_unknown_mode() {
        let toml = FULL_TOML.replace(
            "[window.auth_events]\nmode = \"local\"",
            "[window.auth_events]\nmode = \"distributed\"",
        );
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn reject_window_exceeds_total() {
        // Set max_total_bytes very small so a window exceeds it.
        let toml = FULL_TOML.replace("max_total_bytes = \"2GB\"", "max_total_bytes = \"32MB\"");
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn missing_sources_fails() {
        let toml = r#"
mode = "daemon"
sinks = "sinks"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "schemas/*.wfs"
rules   = "rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"
"#;
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn batch_mode_accepts_file_source() {
        let toml = FULL_TOML
            .replace("mode = \"daemon\"", "mode = \"batch\"")
            .replace(
                "[[sources]]\ntype = \"tcp\"\nname = \"ingress\"\nlisten = \"tcp://127.0.0.1:9800\"\n",
                "[[sources]]\ntype = \"file\"\nname = \"seed_file\"\npath = \"data/auth_events.ndjson\"\nstream = \"syslog\"\nformat = \"ndjson\"\n",
            );
        let cfg: FusionConfig = toml.parse().unwrap();
        assert_eq!(cfg.mode, FusionMode::Batch);
        assert_eq!(cfg.sources.len(), 1);
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.name.as_deref(), Some("seed_file"));
                assert_eq!(file.path, "data/auth_events.ndjson");
                assert_eq!(file.stream, "syslog");
                assert_eq!(file.format, FileInputFormat::Ndjson);
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }
    }

    #[test]
    fn batch_mode_rejects_tcp_source() {
        let toml = FULL_TOML.replace("mode = \"daemon\"", "mode = \"batch\"");
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn daemon_mode_accepts_file_source() {
        let toml = FULL_TOML.replace(
            "[[sources]]\ntype = \"tcp\"\nname = \"ingress\"\nlisten = \"tcp://127.0.0.1:9800\"\n",
            "[[sources]]\ntype = \"file\"\nname = \"seed_file\"\npath = \"data/auth_events.ndjson\"\nstream = \"syslog\"\nformat = \"ndjson\"\n",
        );
        let cfg: FusionConfig = toml.parse().unwrap();
        assert_eq!(cfg.mode, FusionMode::Daemon);
        assert_eq!(cfg.sources.len(), 1);
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.name.as_deref(), Some("seed_file"));
                assert_eq!(file.path, "data/auth_events.ndjson");
                assert_eq!(file.stream, "syslog");
                assert_eq!(file.format, FileInputFormat::Ndjson);
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }
    }

    #[test]
    fn load_with_vars() {
        let toml = format!(
            r#"{}
[vars]
FAIL_THRESHOLD = "5"
SCAN_THRESHOLD = "10"
"#,
            FULL_TOML
        );
        let cfg: FusionConfig = toml.parse().unwrap();
        assert_eq!(cfg.vars.len(), 2);
        assert_eq!(cfg.vars["FAIL_THRESHOLD"], "5");
        assert_eq!(cfg.vars["SCAN_THRESHOLD"], "10");
    }

    #[test]
    fn config_strings_expand_from_vars() {
        let toml = r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"
work_root = "${CASE_PATH}"

[[sources]]
type = "file"
name = "seed_${ENV}"
path = "${CASE_PATH}/data/input.ndjson"
stream = "${STREAM_NAME}"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "${CASE_PATH}/models/schemas/*.wfs"
rules = "${CASE_PATH}/models/rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.conn_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
CASE_PATH = "/tmp/case-a"
ENV = "dev"
STREAM_NAME = "netflow"
"#;
        let cfg: FusionConfig = toml.parse().unwrap();
        assert_eq!(cfg.sinks, "/tmp/case-a/sinks");
        assert_eq!(cfg.work_root.as_deref(), Some("/tmp/case-a"));
        assert_eq!(cfg.runtime.schemas, "/tmp/case-a/models/schemas/*.wfs");
        assert_eq!(cfg.runtime.rules, "/tmp/case-a/models/rules/*.wfl");
        assert_eq!(cfg.vars["CASE_PATH"], "/tmp/case-a");
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.name.as_deref(), Some("seed_dev"));
                assert_eq!(file.path, "/tmp/case-a/data/input.ndjson");
                assert_eq!(file.stream, "netflow");
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }
    }

    #[test]
    fn config_strings_expand_from_environment() {
        let toml = r#"
mode = "batch"
sinks = "${WF_CONFIG_TEST_CASE_PATH}/sinks"

[[sources]]
type = "file"
path = "${WF_CONFIG_TEST_CASE_PATH}/data/input.ndjson"
stream = "netflow"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "${WF_CONFIG_TEST_CASE_PATH}/models/schemas/*.wfs"
rules = "${WF_CONFIG_TEST_CASE_PATH}/models/rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.conn_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"
"#;
        unsafe {
            std::env::set_var("WF_CONFIG_TEST_CASE_PATH", "/tmp/case-env");
        }
        let cfg: FusionConfig = toml.parse().unwrap();
        unsafe {
            std::env::remove_var("WF_CONFIG_TEST_CASE_PATH");
        }

        assert_eq!(cfg.sinks, "/tmp/case-env/sinks");
        assert_eq!(cfg.runtime.schemas, "/tmp/case-env/models/schemas/*.wfs");
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.path, "/tmp/case-env/data/input.ndjson");
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }
    }

    #[test]
    fn explicit_vars_override_file_vars_and_expose_builtins() {
        let root = make_temp_dir("context-vars");
        let config_path = root.join("conf/wfusion.toml");
        let work_dir = root.join("workspace");
        std::fs::create_dir_all(&work_dir).expect("failed to create work dir");
        write_file(
            &config_path,
            r#"
mode = "batch"
sinks = "${CASE_PATH}/sinks"
work_root = "${WORK_DIR}/out"

[[sources]]
type = "file"
name = "seed"
path = "${CONFIG_DIR}/data/input.ndjson"
stream = "netflow"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "${CONFIG_DIR}/models/schemas/*.wfs"
rules = "${WORK_DIR}/rules/*.wfl"

[window_defaults]
evict_interval = "30s"
max_window_bytes = "256MB"
max_total_bytes = "2GB"
evict_policy = "time_first"
watermark = "5s"
allowed_lateness = "0s"
late_policy = "drop"

[window.conn_events]
mode = "local"
max_window_bytes = "256MB"
over_cap = "30m"

[vars]
CASE_PATH = "/tmp/from-file"
"#,
        );

        let mut explicit_vars = HashMap::new();
        explicit_vars.insert("CASE_PATH".to_string(), "/tmp/from-cli".to_string());
        let ctx = ConfigVarContext::from_explicit_vars(explicit_vars);
        let cfg = FusionConfig::load_with_context(&config_path, &ctx, Some(&work_dir)).unwrap();

        assert_eq!(cfg.sinks, "/tmp/from-cli/sinks");
        assert_eq!(
            cfg.work_root.as_deref(),
            Some(work_dir.join("out").to_string_lossy().as_ref())
        );
        assert_eq!(
            cfg.runtime.schemas,
            config_path
                .parent()
                .expect("config dir")
                .join("models/schemas/*.wfs")
                .to_string_lossy()
        );
        assert_eq!(
            cfg.runtime.rules,
            work_dir.join("rules/*.wfl").to_string_lossy()
        );
        assert_eq!(cfg.vars["CASE_PATH"], "/tmp/from-cli");
        assert_eq!(
            cfg.vars["CONFIG_DIR"],
            config_path.parent().unwrap().to_string_lossy()
        );
        assert_eq!(cfg.vars["WORK_DIR"], work_dir.to_string_lossy());

        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(
                    file.path,
                    config_path
                        .parent()
                        .unwrap()
                        .join("data/input.ndjson")
                        .to_string_lossy()
                );
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_with_overlays_merges_tables_and_replaces_arrays() {
        let root = make_temp_dir("overlay-merge");
        let base_path = root.join("conf/base.toml");
        let overlay_path = root.join("conf/overlay.toml");
        write_file(
            &base_path,
            r#"
mode = "daemon"
sinks = "base_sinks"

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
name = "seed_file"
path = "data/seed.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
rules = "rules/overlay/*.wfl"

[window.overlay_events]
mode = "replicated"
max_window_bytes = "64MB"
over_cap = "48h"
"#,
        );

        let cfg = FusionConfig::load_with_overlays(
            &base_path,
            &[overlay_path],
            &ConfigVarContext::new(),
            None,
        )
        .expect("load with overlays");
        assert_eq!(cfg.mode, FusionMode::Batch);
        assert_eq!(cfg.sinks, "base_sinks");
        assert_eq!(cfg.runtime.schemas, "schemas/base/*.wfs");
        assert_eq!(cfg.runtime.rules, "rules/overlay/*.wfl");
        assert_eq!(cfg.windows.len(), 2);
        assert!(cfg.windows.iter().any(|w| w.name == "base_events"));
        assert!(cfg.windows.iter().any(|w| w.name == "overlay_events"));
        assert_eq!(cfg.sources.len(), 1);
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.path, "data/seed.ndjson");
                assert_eq!(file.stream, "syslog");
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_with_overlays_allows_overlay_vars_to_override_base_vars() {
        let root = make_temp_dir("overlay-vars");
        let base_path = root.join("conf/base.toml");
        let overlay_path = root.join("conf/overlay.toml");
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
schemas = "${CASE_PATH}/schemas/*.wfs"
rules = "${CASE_PATH}/rules/*.wfl"

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
[vars]
CASE_PATH = "/tmp/overlay"
"#,
        );

        let cfg = FusionConfig::load_with_overlays(
            &base_path,
            &[overlay_path],
            &ConfigVarContext::new(),
            None,
        )
        .expect("load with overlays");
        assert_eq!(cfg.sinks, "/tmp/overlay/sinks");
        assert_eq!(cfg.runtime.schemas, "/tmp/overlay/schemas/*.wfs");
        assert_eq!(cfg.vars["CASE_PATH"], "/tmp/overlay");
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.path, "/tmp/overlay/data/base.ndjson");
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_with_overlays_rebases_overlay_relative_paths_against_base_config_dir() {
        let root = make_temp_dir("overlay-rebase-config-dir");
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
work_root = "../out/dev"

[[sources]]
type = "file"
path = "../data/dev.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
schemas = "../schemas/dev/*.wfs"
rules = "../rules/dev/*.wfl"

[logging]
file = "../logs/dev.log"
"#,
        );

        let cfg = FusionConfig::load_with_overlays(
            &base_path,
            &[overlay_path],
            &ConfigVarContext::new(),
            None,
        )
        .expect("load with overlays");
        assert_eq!(cfg.sinks, "../env/sinks/dev");
        assert_eq!(cfg.work_root.as_deref(), Some("../env/out/dev"));
        assert_eq!(cfg.runtime.schemas, "../env/schemas/dev/*.wfs");
        assert_eq!(cfg.runtime.rules, "../env/rules/dev/*.wfl");
        assert_eq!(
            cfg.logging
                .file
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            Some("../env/logs/dev.log".to_string())
        );
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.path, "../env/data/dev.ndjson");
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn load_with_overlays_rebases_overlay_relative_paths_against_work_dir_when_provided() {
        let root = make_temp_dir("overlay-rebase-work-dir");
        let work_dir = root.join("workspace");
        let base_path = work_dir.join("conf/base.toml");
        let overlay_path = work_dir.join("env/dev/overlay.toml");
        std::fs::create_dir_all(&work_dir).expect("failed to create work dir");
        write_file(
            &base_path,
            r#"
mode = "batch"
sinks = "conf/sinks"

[[sources]]
type = "file"
path = "conf/data/base.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
executor_parallelism = 2
rule_exec_timeout = "30s"
schemas = "conf/schemas/base/*.wfs"
rules = "conf/rules/base/*.wfl"

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

[[sources]]
type = "file"
path = "../data/dev.ndjson"
stream = "syslog"
format = "ndjson"

[runtime]
rules = "../rules/dev/*.wfl"
"#,
        );

        let ctx = ConfigVarContext::new();
        let cfg =
            FusionConfig::load_with_overlays(&base_path, &[overlay_path], &ctx, Some(&work_dir))
                .expect("load with overlays");
        assert_eq!(cfg.sinks, "env/sinks/dev");
        assert_eq!(cfg.runtime.rules, "env/rules/dev/*.wfl");
        match &cfg.sources[0] {
            SourceConfig::File(file) => {
                assert_eq!(file.path, "env/data/dev.ndjson");
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn reject_cyclic_config_vars() {
        let toml = format!(
            r#"{}
[vars]
A = "${{B}}"
B = "${{A}}"
"#,
            FULL_TOML.replace("mode = \"daemon\"", "mode = \"batch\"").replace(
                "[[sources]]\ntype = \"tcp\"\nname = \"ingress\"\nlisten = \"tcp://127.0.0.1:9800\"\n",
                "[[sources]]\ntype = \"file\"\nname = \"seed_file\"\npath = \"data/auth_events.ndjson\"\nstream = \"syslog\"\nformat = \"ndjson\"\n",
            )
        );
        let err = toml.parse::<FusionConfig>().unwrap_err();
        assert!(
            err.to_string().contains("cyclic variable reference"),
            "error should mention cycle: {err}",
        );
    }

    #[test]
    fn reject_invalid_var_name_hyphen() {
        let toml = format!(
            r#"{}
[vars]
my-var = "value"
"#,
            FULL_TOML
        );
        let err = toml.parse::<FusionConfig>().unwrap_err();
        assert!(
            err.to_string().contains("my-var"),
            "error should mention the bad key: {err}",
        );
    }

    #[test]
    fn reject_invalid_var_name_digit_start() {
        let toml = format!(
            r#"{}
[vars]
1BAD = "value"
"#,
            FULL_TOML
        );
        let err = toml.parse::<FusionConfig>().unwrap_err();
        assert!(
            err.to_string().contains("1BAD"),
            "error should mention the bad key: {err}",
        );
    }

    #[test]
    fn accept_underscore_var_name() {
        let toml = format!(
            r#"{}
[vars]
_PRIVATE = "ok"
MAX_COUNT_2 = "99"
"#,
            FULL_TOML
        );
        let cfg: FusionConfig = toml.parse().unwrap();
        assert_eq!(cfg.vars["_PRIVATE"], "ok");
        assert_eq!(cfg.vars["MAX_COUNT_2"], "99");
    }

    #[test]
    fn load_with_metrics_block() {
        let toml = format!(
            r#"{}
[metrics]
enabled = true
report_interval = "5s"
prometheus_listen = "127.0.0.1:19001"

[metrics.topn]
enabled = true
max = 50
queue_capacity = 8192
"#,
            FULL_TOML
        );
        let cfg: FusionConfig = toml.parse().unwrap();
        assert!(cfg.metrics.enabled);
        assert_eq!(
            cfg.metrics.report_interval.as_duration(),
            Duration::from_secs(5)
        );
        assert_eq!(cfg.metrics.prometheus_listen, "127.0.0.1:19001");
        assert!(cfg.metrics.topn.enabled);
        assert_eq!(cfg.metrics.topn.max, 50);
        assert_eq!(cfg.metrics.topn.queue_capacity, 8192);
    }

    #[test]
    fn reject_invalid_metrics_listen() {
        let toml = format!(
            r#"{}
[metrics]
enabled = true
prometheus_listen = "not-a-socket"
"#,
            FULL_TOML
        );
        assert!(toml.parse::<FusionConfig>().is_err());
    }

    #[test]
    fn load_explicit_sources() {
        let toml = format!(
            r#"{}
[[sources]]
type = "file"
name = "seed_file"
path = "data/auth_events.ndjson"
stream = "syslog"
format = "ndjson"
"#,
            FULL_TOML
        );
        let cfg: FusionConfig = toml.parse().unwrap();
        assert_eq!(cfg.sources.len(), 2);
        match &cfg.sources[0] {
            SourceConfig::Tcp(tcp) => {
                assert_eq!(tcp.name.as_deref(), Some("ingress"));
                assert_eq!(tcp.listen, "tcp://127.0.0.1:9800");
            }
            SourceConfig::File(_) => panic!("expected tcp source"),
        }
        match &cfg.sources[1] {
            SourceConfig::File(file) => {
                assert_eq!(file.name.as_deref(), Some("seed_file"));
                assert_eq!(file.path, "data/auth_events.ndjson");
                assert_eq!(file.stream, "syslog");
                assert_eq!(file.format, FileInputFormat::Ndjson);
            }
            SourceConfig::Tcp(_) => panic!("expected file source"),
        }
    }
}
