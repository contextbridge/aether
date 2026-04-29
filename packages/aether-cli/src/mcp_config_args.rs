use aether_core::agent_spec::McpConfigSource;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct McpConfigArgs {
    #[arg(long = "mcp-config", value_name = "PATH")]
    pub mcp_configs: Vec<PathBuf>,

    #[arg(long = "mcp-config-json", value_name = "JSON")]
    pub mcp_config_jsons: Vec<String>,
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
