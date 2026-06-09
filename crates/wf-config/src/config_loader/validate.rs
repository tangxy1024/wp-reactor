use std::collections::HashMap;
use std::net::ToSocketAddrs;
use std::time::Duration;

use orion_error::conversion::SourceErr;

use crate::error::{ConfigReason, ConfigResult};
use crate::config_loader::fusion::{FusionConfig, FusionMode};
use crate::source::SourceConfig;
use crate::window::WindowConfig;

/// Internal validation, called automatically during `FusionConfig::from_str` / `load`.
pub(crate) fn validate(config: &FusionConfig) -> ConfigResult<()> {
    // runtime.executor_parallelism > 0
    if config.runtime.executor_parallelism == 0 {
        return ConfigReason::Validation.fail("runtime.executor_parallelism must be > 0");
    }

    // Each window's max_window_bytes ≤ window_defaults.max_total_bytes
    let max_total = config.window_defaults.max_total_bytes.as_bytes();
    for w in &config.windows {
        if w.max_window_bytes.as_bytes() > max_total {
            return ConfigReason::Validation.fail(format!(
                "window {:?}: max_window_bytes ({}) exceeds window_defaults.max_total_bytes ({})",
                w.name, w.max_window_bytes, config.window_defaults.max_total_bytes,
            ));
        }
    }

    // vars keys must be valid WFL identifiers: [A-Za-z_][A-Za-z0-9_]*
    for key in config.vars.keys() {
        if !is_valid_var_name(key) {
            return ConfigReason::Validation.fail(format!(
                "vars: invalid variable name {:?} - must match [A-Za-z_][A-Za-z0-9_]*",
                key,
            ));
        }
    }

    // sinks path must be non-empty
    if config.sinks.is_empty() {
        return ConfigReason::Validation
            .fail("sinks must be a non-empty path to the sinks/ directory");
    }

    // sources validation
    if config.sources.is_empty() {
        return ConfigReason::Validation.fail("at least one source is required");
    }
    let mut names = std::collections::HashSet::new();
    let mut enabled_tcp = 0usize;
    let mut enabled_file = 0usize;
    for (idx, source) in config.sources.iter().enumerate() {
        let name = source.effective_name(idx);
        if !names.insert(name.clone()) {
            return ConfigReason::Validation.fail(format!("duplicate source name: {name:?}"));
        }
        match source {
            SourceConfig::Tcp(tcp) => {
                if tcp.enabled {
                    enabled_tcp += 1;
                }
                if !tcp.listen.starts_with("tcp://") {
                    return ConfigReason::Validation.fail(format!(
                        "sources[{idx}] ({name}): tcp listen must start with \"tcp://\", got {:?}",
                        tcp.listen
                    ));
                }
            }
            SourceConfig::File(file) => {
                if file.enabled {
                    enabled_file += 1;
                }
                if file.path.trim().is_empty() {
                    return ConfigReason::Validation.fail(format!(
                        "sources[{idx}] ({name}): file path must be non-empty"
                    ));
                }
                if matches!(
                    file.format,
                    crate::source::FileInputFormat::Ndjson
                        | crate::source::FileInputFormat::ArrowIpc
                ) && file.stream.trim().is_empty()
                {
                    return ConfigReason::Validation.fail(format!(
                        "sources[{idx}] ({name}): file stream must be non-empty"
                    ));
                }
            }
        }
    }
    match config.mode {
        FusionMode::Daemon => {
            if enabled_tcp + enabled_file == 0 {
                return ConfigReason::Validation
                    .fail("daemon mode requires at least one enabled source");
            }
        }
        FusionMode::Batch => {
            if enabled_file == 0 {
                return ConfigReason::Validation
                    .fail("batch mode requires at least one enabled file source");
            }
            if enabled_tcp > 0 {
                return ConfigReason::Validation
                    .fail("batch mode does not allow enabled tcp sources");
            }
        }
    }

    // metrics config sanity
    if config.metrics.report_interval.as_duration().is_zero() {
        return ConfigReason::Validation.fail("metrics.report_interval must be > 0");
    }
    if config.metrics.topn.max == 0 {
        return ConfigReason::Validation.fail("metrics.topn.max must be > 0");
    }
    if config.metrics.topn.queue_capacity == 0 {
        return ConfigReason::Validation.fail("metrics.topn.queue_capacity must be > 0");
    }
    if config.metrics.enabled {
        if config.metrics.prometheus_listen.trim().is_empty() {
            return ConfigReason::Validation
                .fail("metrics.prometheus_listen must be non-empty when metrics.enabled=true");
        }
        // Must be host:port (no scheme).
        if config
            .metrics
            .prometheus_listen
            .to_socket_addrs()
            .source_err(
                ConfigReason::Validation,
                "metrics.prometheus_listen invalid",
            )?
            .next()
            .is_none()
        {
            return ConfigReason::Validation
                .fail("metrics.prometheus_listen resolved to no socket address");
        }
    }

    Ok(())
}

/// A valid variable name starts with ASCII letter or underscore, followed by
/// ASCII alphanumerics or underscores.
fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.bytes();
    match chars.next() {
        Some(b) if b.is_ascii_alphabetic() || b == b'_' => {}
        _ => return false,
    }
    chars.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Cross-file validation: check that every window's `.wfs` `over` duration does not exceed
/// the `over_cap` configured in `wfusion.toml`.
///
/// Call this after loading both the config and the `.wfs` schema files.
///
/// - `windows`: resolved window configs from `FusionConfig`.
/// - `window_overs`: map of window name → `over` duration parsed from `.wfs` files.
pub fn validate_over_vs_over_cap(
    windows: &[WindowConfig],
    window_overs: &HashMap<String, Duration>,
) -> ConfigResult<()> {
    for (name, over) in window_overs {
        let Some(wc) = windows.iter().find(|w| w.name == *name) else {
            return ConfigReason::Validation.fail(format!(
                "window {name:?} found in .wfs schema but not in wfusion.toml [window.{name}]"
            ));
        };
        let cap: Duration = wc.over_cap.into();
        if *over > cap {
            return ConfigReason::Validation.fail(format!(
                "window {name:?}: over ({over:?}) exceeds over_cap ({cap:?})",
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ByteSize, DistMode, EvictPolicy, HumanDuration, LatePolicy};

    fn sample_window(name: &str, over_cap_secs: u64) -> WindowConfig {
        WindowConfig {
            name: name.to_string(),
            mode: DistMode::Local,
            max_window_bytes: ByteSize::from(256 * 1024 * 1024),
            over_cap: HumanDuration::from(Duration::from_secs(over_cap_secs)),
            evict_policy: EvictPolicy::TimeFirst,
            watermark: HumanDuration::from(Duration::from_secs(5)),
            allowed_lateness: HumanDuration::from(Duration::from_secs(0)),
            late_policy: LatePolicy::Drop,
            table: None,
        }
    }

    #[test]
    fn over_vs_over_cap_accept() {
        let windows = vec![sample_window("auth_events", 1800)]; // 30m
        let mut overs = HashMap::new();
        overs.insert("auth_events".into(), Duration::from_secs(300)); // 5m ≤ 30m
        assert!(validate_over_vs_over_cap(&windows, &overs).is_ok());
    }

    #[test]
    fn over_vs_over_cap_reject() {
        let windows = vec![sample_window("auth_events", 1800)]; // 30m
        let mut overs = HashMap::new();
        overs.insert("auth_events".into(), Duration::from_secs(3600)); // 60m > 30m
        let err = validate_over_vs_over_cap(&windows, &overs).unwrap_err();
        assert!(err.to_string().contains("auth_events"));
    }

    #[test]
    fn over_vs_over_cap_missing_window() {
        let windows = vec![sample_window("auth_events", 1800)];
        let mut overs = HashMap::new();
        overs.insert("unknown_window".into(), Duration::from_secs(300));
        assert!(validate_over_vs_over_cap(&windows, &overs).is_err());
    }
}
