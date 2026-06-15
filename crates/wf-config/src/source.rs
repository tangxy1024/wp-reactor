use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single `[[sources]]` entry.
///
/// Supports two formats:
///
/// 1. Legacy flat format:
/// ```toml
/// type = "file"
/// key = "netflow_file"
/// enabled = true
/// path = "data/events.ndjson"
/// stream = "netflow"
/// ```
///
/// 2. Standard connector format:
/// ```toml
/// connect = "kafka_src"
/// key = "kafka_1"
/// stream = "nginx_access"
/// brokers = "localhost:9092"
/// topic = "wp_nginx_logs"
/// ```
///
/// When `connect` is set, `type` is optional — the kind is resolved from the
/// connector registry at runtime via [`SourceConfig::resolve_kind`].
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct SourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    /// Legacy: direct source kind (e.g. `"file"`, `"tcp"`, `"kafka"`).
    #[serde(rename = "type", default)]
    pub source_type: Option<String>,
    /// Standard: connector id (e.g. `"kafka_src"`, `"file_src"`).
    /// Resolved to a kind via the connector registry.
    #[serde(default)]
    pub connect: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// All other fields (flat or under `[sources.params]`) are captured here.
    #[serde(default, flatten)]
    pub params: BTreeMap<String, String>,
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            name: None,
            source_type: None,
            connect: None,
            enabled: true,
            params: BTreeMap::new(),
        }
    }
}

impl SourceConfig {
    pub fn effective_name(&self, index: usize) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{}_{}", self.kind(), index + 1))
    }

    /// Return the effective source kind.
    ///
    /// Priority: `source_type` > `connect` derived kind > `"unknown"`.
    /// For standard connector ids (`<kind>_src` → `<kind>`), the kind is
    /// derived directly without needing the connector registry.
    pub fn kind(&self) -> &str {
        if let Some(t) = self.source_type.as_deref() {
            return t;
        }
        if let Some(conn) = self.connect.as_deref() {
            if let Some(kind) = conn.strip_suffix("_src") {
                return kind;
            }
            // Return the connector id itself as fallback
            return conn;
        }
        "unknown"
    }

    /// Resolve `connect` → kind via a lookup function, storing the result in
    /// `source_type`. Returns the resolved kind.
    ///
    /// The lookup function receives a connector id (e.g. `"kafka_src"`) and
    /// should return the corresponding kind (e.g. `"kafka"`), or `None` if
    /// the connector is not found.
    pub fn resolve_kind(&mut self, lookup: impl Fn(&str) -> Option<String>) -> Option<&str> {
        if self.source_type.is_none()
            && let Some(ref conn) = self.connect
            && let Some(kind) = lookup(conn)
        {
            self.source_type = Some(kind);
        }
        self.source_type.as_deref()
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_flat_format() {
        let toml = r#"
type = "file"
key = "netflow_file"
path = "data/events.ndjson"
stream = "netflow"
format = "ndjson"
"#;
        let s: SourceConfig = toml::from_str(toml).unwrap();
        assert_eq!(s.kind(), "file");
        assert_eq!(s.name.as_deref(), Some("netflow_file"));
        assert_eq!(s.params.get("path").unwrap(), "data/events.ndjson");
        assert!(s.connect.is_none());
    }

    #[test]
    fn parse_connector_format_with_flat_params() {
        let toml = r#"
key = "kafka_1"
connect = "kafka_src"
stream = "nginx_access"
brokers = "localhost:9092"
topic = "wp_nginx_logs"
group_id = "wfusion"
"#;
        let s: SourceConfig = toml::from_str(toml).unwrap();
        assert_eq!(s.name.as_deref(), Some("kafka_1"));
        assert_eq!(s.connect.as_deref(), Some("kafka_src"));
        assert!(s.source_type.is_none());
        assert_eq!(s.params.get("brokers").unwrap(), "localhost:9092");
        assert_eq!(s.params.get("topic").unwrap(), "wp_nginx_logs");
        assert_eq!(s.params.get("stream").unwrap(), "nginx_access");
    }

    #[test]
    fn resolve_kind_from_connect() {
        let mut s = SourceConfig {
            name: Some("kafka_1".into()),
            source_type: None,
            connect: Some("kafka_src".into()),
            ..Default::default()
        };
        // kind() derives from connect before explicit resolution
        assert_eq!(s.kind(), "kafka");
        // Explicit resolution also works (e.g. for non-standard connector ids)
        let kind = s.resolve_kind(|id| {
            if id == "kafka_src" {
                Some("kafka".into())
            } else {
                None
            }
        });
        assert_eq!(kind, Some("kafka"));
        assert_eq!(s.source_type.as_deref(), Some("kafka"));
    }

    #[test]
    fn resolve_kind_keeps_existing_type() {
        let mut s = SourceConfig {
            source_type: Some("tcp".into()),
            ..Default::default()
        };
        let kind = s.resolve_kind(|_| panic!("should not be called"));
        assert_eq!(kind, Some("tcp"));
    }

    #[test]
    fn kind_derived_from_connect_file_src() {
        let s = SourceConfig {
            connect: Some("file_src".into()),
            ..Default::default()
        };
        assert_eq!(s.kind(), "file");
    }

    #[test]
    fn kind_derived_from_connect_tcp_src() {
        let s = SourceConfig {
            connect: Some("tcp_src".into()),
            ..Default::default()
        };
        assert_eq!(s.kind(), "tcp");
    }

    #[test]
    fn kind_returns_connect_when_no_src_suffix() {
        let s = SourceConfig {
            connect: Some("custom_connector".into()),
            ..Default::default()
        };
        assert_eq!(s.kind(), "custom_connector");
    }

    #[test]
    fn parse_tcp_legacy_format() {
        let toml = r#"
type = "tcp"
listen = "tcp://0.0.0.0:9800"
"#;
        let s: SourceConfig = toml::from_str(toml).unwrap();
        assert_eq!(s.kind(), "tcp");
        assert_eq!(s.params.get("listen").unwrap(), "tcp://0.0.0.0:9800");
    }
}
