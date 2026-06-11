use serde::{Deserialize, Serialize};

// -- Source config (aligned with wp-core-connectors field names) -------------

#[derive(
    ::moju_derive::MoJu, Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default,
)]
#[serde(rename_all = "snake_case")]
#[moju(kind = "state", domain = "Config", module = "Config.SourceConfig")]
pub enum FileInputFormat {
    #[default]
    Ndjson,
    Csv,
    ArrowFramed,
    ArrowIpc,
}

/// `[[sources]]` entry — connector-based format aligned with wp-core-connectors.
///
/// Uses `connect` (connector kind) instead of old `type` tag.
///
/// ```toml
/// [[sources]]
/// key = "netflow_file"          # was: name (still accepts "name" for backward compat)
/// connect = "file"              # was: type = "file"
/// path = "data/events.ndjson"
/// stream = "netflow"
/// format = "ndjson"
/// enabled = true
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
    pub listen: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct FileSourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    pub path: String,
    #[serde(default)]
    pub stream: String,
    #[serde(default)]
    pub format: FileInputFormat,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct KafkaSourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    pub brokers: Vec<String>,
    pub topic: String,
    #[serde(default = "default_group_id")]
    pub group_id: String,
    #[serde(default)]
    pub format: FileInputFormat,
    #[serde(default)]
    pub stream: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_group_id() -> String {
    "wfusion".into()
}

fn default_true() -> bool {
    true
}

impl SourceConfig {
    pub fn effective_name(&self, index: usize) -> String {
        match self {
            SourceConfig::Tcp(tcp) => {
                tcp.name.clone().unwrap_or_else(|| format!("tcp_{}", index + 1))
            }
            SourceConfig::File(file) => {
                file.name.clone().unwrap_or_else(|| format!("file_{}", index + 1))
            }
            SourceConfig::Kafka(kafka) => {
                kafka.name.clone().unwrap_or_else(|| format!("kafka_{}", index + 1))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_connector_format() {
        let toml = r#"
key = "netflow_file"
connect = "file"
path = "data/events.ndjson"
stream = "netflow"
format = "ndjson"
"#;
        let source: SourceConfig = toml::from_str(toml).unwrap();
        assert!(matches!(source, SourceConfig::File(_)));
        assert_eq!(source.effective_name(0), "netflow_file");
    }

    #[test]
    fn parse_tcp_connector_format() {
        let toml = r#"
connect = "tcp"
listen = "tcp://0.0.0.0:9800"
"#;
        let source: SourceConfig = toml::from_str(toml).unwrap();
        assert!(matches!(source, SourceConfig::Tcp(_)));
        assert_eq!(source.effective_name(0), "tcp_1");
    }

    #[test]
    fn parse_kafka_connector_format() {
        let toml = r#"
connect = "kafka"
brokers = ["localhost:9092"]
topic = "netflow"
"#;
        let source: SourceConfig = toml::from_str(toml).unwrap();
        assert!(matches!(source, SourceConfig::Kafka(_)));
    }
}
