use serde::{Deserialize, Serialize};

#[derive(::moju_derive::MoJu, Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
#[moju(kind = "state", domain = "Config", module = "Config.SourceConfig")]
pub enum FileInputFormat {
    #[default]
    Ndjson,
    Csv,
    ArrowFramed,
    ArrowIpc,
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[moju(kind = "state", domain = "Config", module = "Config.SourceConfig")]
pub enum SourceConfig {
    Tcp(TcpSourceConfig),
    File(FileSourceConfig),
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct TcpSourceConfig {
    #[serde(default)]
    pub name: Option<String>,
    pub listen: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(::moju_derive::MoJu, Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[moju(kind = "struct", domain = "Config", module = "Config.SourceConfig")]
pub struct FileSourceConfig {
    #[serde(default)]
    pub name: Option<String>,
    pub path: String,
    #[serde(default)]
    pub stream: String,
    #[serde(default)]
    pub format: FileInputFormat,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl SourceConfig {
    pub fn effective_name(&self, index: usize) -> String {
        match self {
            SourceConfig::Tcp(tcp) => tcp
                .name
                .clone()
                .unwrap_or_else(|| format!("tcp_{}", index + 1)),
            SourceConfig::File(file) => file
                .name
                .clone()
                .unwrap_or_else(|| format!("file_{}", index + 1)),
        }
    }
}
