use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// -- Source config (aligned with wp-core-connectors params pattern) ----------

/// `[[sources]]` entry.
///
/// ```toml
/// [[sources]]
/// type = "file"              # connector kind
/// key = "netflow_file"       # optional identifier
/// enabled = true
/// [sources.params]
/// path = "data/events.ndjson"
/// stream = "netflow"
/// format = "ndjson"
/// ```
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[moju(kind = "state", domain = "Config", module = "Config.SourceConfig")]
pub enum SourceConfig {
    #[serde(rename = "tcp")]
    Tcp(TcpSourceConfig),
    #[serde(rename = "file")]
    File(FileSourceConfig),
    #[serde(rename = "kafka")]
    Kafka(KafkaSourceConfig),
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct TcpSourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Connector-specific params (listen, stream, etc.)
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct FileSourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct KafkaSourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

// -- Accessors ---------------------------------------------------------------

impl TcpSourceConfig {
    pub fn listen(&self) -> &str {
        self.params.get("listen").map(|s| s.as_str()).unwrap_or("")
    }
}

impl FileSourceConfig {
    pub fn path(&self) -> &str {
        self.params.get("path").map(|s| s.as_str()).unwrap_or("")
    }
    pub fn stream(&self) -> &str {
        self.params.get("stream").map(|s| s.as_str()).unwrap_or("")
    }
    pub fn format(&self) -> &str {
        self.params.get("format").map(|s| s.as_str()).unwrap_or("ndjson")
    }
}

impl KafkaSourceConfig {
    pub fn brokers(&self) -> Vec<String> {
        self.params.get("brokers")
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default()
    }
    pub fn topic(&self) -> &str {
        self.params.get("topic").map(|s| s.as_str()).unwrap_or("")
    }
    pub fn group_id(&self) -> &str {
        self.params.get("group_id").map(|s| s.as_str()).unwrap_or("wfusion")
    }
    pub fn stream(&self) -> &str {
        self.params.get("stream").map(|s| s.as_str()).unwrap_or("")
    }
    pub fn format(&self) -> &str {
        self.params.get("format").map(|s| s.as_str()).unwrap_or("ndjson")
    }
}

impl SourceConfig {
    pub fn effective_name(&self, index: usize) -> String {
        match self {
            SourceConfig::Tcp(t) => t.name.clone().unwrap_or_else(|| format!("tcp_{}", index + 1)),
            SourceConfig::File(f) => f.name.clone().unwrap_or_else(|| format!("file_{}", index + 1)),
            SourceConfig::Kafka(k) => k.name.clone().unwrap_or_else(|| format!("kafka_{}", index + 1)),
        }
    }
}

fn default_true() -> bool { true }

// -- Tests -------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_params_format() {
        let toml = r#"
type = "file"
key = "netflow_file"

[params]
path = "data/events.ndjson"
stream = "netflow"
format = "ndjson"
"#;
        let source: SourceConfig = toml::from_str(toml).unwrap();
        assert!(matches!(source, SourceConfig::File(_)));
        if let SourceConfig::File(f) = &source {
            assert_eq!(f.name.as_deref(), Some("netflow_file"));
            assert_eq!(f.path(), "data/events.ndjson");
            assert_eq!(f.stream(), "netflow");
            assert_eq!(f.format(), "ndjson");
        }
    }

    #[test]
    fn parse_tcp_params_format() {
        let toml = r#"
type = "tcp"

[params]
listen = "tcp://0.0.0.0:9800"
"#;
        let source: SourceConfig = toml::from_str(toml).unwrap();
        if let SourceConfig::Tcp(t) = &source {
            assert_eq!(t.listen(), "tcp://0.0.0.0:9800");
        }
    }
}
