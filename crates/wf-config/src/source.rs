use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A single `[[sources]]` entry.
///
/// ```toml
/// [[sources]]
/// type = "file"
/// key = "netflow_file"
/// enabled = true
///
/// [sources.params]
/// path = "data/events.ndjson"
/// stream = "netflow"
/// format = "ndjson"
/// ```
#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct SourceConfig {
    #[serde(default, alias = "key")]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub source_type: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
}

impl SourceConfig {
    pub fn effective_name(&self, index: usize) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("{}_{}", self.source_type, index + 1))
    }

    pub fn kind(&self) -> &str { &self.source_type }
}

fn default_true() -> bool { true }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_file_params_subtable() {
        let toml = r#"
type = "file"
key = "netflow_file"

[params]
path = "data/events.ndjson"
stream = "netflow"
format = "ndjson"
"#;
        let s: SourceConfig = toml::from_str(toml).unwrap();
        assert_eq!(s.kind(), "file");
        assert_eq!(s.name.as_deref(), Some("netflow_file"));
        assert_eq!(s.params.get("path").unwrap(), "data/events.ndjson");
    }

    #[test]
    fn parse_tcp_params_subtable() {
        let toml = r#"
type = "tcp"

[params]
listen = "tcp://0.0.0.0:9800"
"#;
        let s: SourceConfig = toml::from_str(toml).unwrap();
        assert_eq!(s.kind(), "tcp");
        assert_eq!(s.params.get("listen").unwrap(), "tcp://0.0.0.0:9800");
    }
}
