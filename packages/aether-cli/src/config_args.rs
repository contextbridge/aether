use aether_core::agent_spec::McpConfigSource;
use aether_project::AetherConfigSource;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct ConfigSourceArgs {
    #[arg(long = "config-json", conflicts_with = "config_file")]
    pub config_json: Option<String>,

    #[arg(long = "config-file", conflicts_with = "config_json")]
    pub config_file: Option<PathBuf>,
}

#[derive(Clone, Debug, Default, clap::Args)]
pub struct McpConfigArgs {
    #[arg(long = "mcp-config", value_name = "PATH")]
    pub mcp_configs: Vec<PathBuf>,

    #[arg(long = "mcp-config-json", value_name = "JSON")]
    pub mcp_config_jsons: Vec<String>,
}

impl ConfigSourceArgs {
    pub fn source(&self) -> Option<AetherConfigSource> {
        if let Some(json) = &self.config_json {
            Some(AetherConfigSource::Json(json.clone()))
        } else {
            self.config_file.as_ref().map(|path| AetherConfigSource::File(path.clone()))
        }
    }
}

impl McpConfigArgs {
    pub fn sources(&self, project_root: &Path) -> Vec<McpConfigSource> {
        self.mcp_configs
            .iter()
            .map(|path| project_root.join(path))
            .map(McpConfigSource::direct)
            .chain(self.mcp_config_jsons.iter().cloned().map(McpConfigSource::Json))
            .collect()
    }
}
