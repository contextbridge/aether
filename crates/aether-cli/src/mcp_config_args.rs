use aether_core::mcp::{McpConfig, ParseError};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, clap::Args)]
pub struct McpConfigArgs {
    #[arg(long = "mcp-config", value_name = "PATH")]
    pub mcp_configs: Vec<PathBuf>,

    #[arg(long = "mcp-config-json", value_name = "JSON")]
    pub mcp_config_jsons: Vec<String>,
}

impl McpConfigArgs {
    pub fn load(&self, project_root: &Path) -> Result<McpConfig, ParseError> {
        let files = self.mcp_configs.iter().map(|path| McpConfig::from_json_file(project_root.join(path)));
        let jsons = self.mcp_config_jsons.iter().map(|json| McpConfig::from_json(json));
        files.chain(jsons).collect()
    }
}
