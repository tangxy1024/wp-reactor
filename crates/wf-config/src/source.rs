use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FileInputFormat {
    #[default]
    Ndjson,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceConfig {
    Tcp(TcpSourceConfig),
    File(FileSourceConfig),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct TcpSourceConfig {
    #[serde(default)]
    pub name: Option<String>,
    pub listen: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct FileSourceConfig {
    #[serde(default)]
    pub name: Option<String>,
    pub path: String,
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
