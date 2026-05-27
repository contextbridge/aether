use aether_auth::OAuthCredentialStorage;

use mcp_utils::client::{
    McpClientEvent, McpConfig, McpConnectionDetails, McpError, McpManager, McpServer, OAuthHandlerFactory, ParseError,
    ServerFactory, root_from_path,
};
use utils::variables::Vars;

use crate::agent_spec::McpConfigSource;

use super::run_mcp_task::{McpCommand, run_mcp_task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::{
    sync::mpsc::{self, Receiver, Sender},
    task::JoinHandle,
};

pub fn mcp(workspace_root: impl AsRef<Path>) -> McpBuilder {
    McpBuilder::new(workspace_root)
}

/// Handle to the spawned MCP manager task. Consumers receive incremental
/// updates over `event_rx` (starting with an initial `ServerStatusesChanged`
/// reflecting every configured server in `Connecting`) and can call
/// `block_until_ready()` to block until every server has finished its initial
/// connection attempt.
pub struct McpSpawnResult {
    pub command_tx: Sender<McpCommand>,
    pub event_rx: Receiver<McpClientEvent>,
    pub handle: JoinHandle<()>,
}

impl McpSpawnResult {
    /// Block until the manager finishes bootstrapping every initially-configured
    /// server, then return the consolidated snapshot. Returns `None` if the
    /// event channel closes before `ConnectionReady` is received.
    pub async fn block_until_ready(&mut self) -> Option<McpConnectionDetails> {
        while let Some(event) = self.event_rx.recv().await {
            if let McpClientEvent::ConnectionReady(snapshot) = event {
                return Some(snapshot);
            }
        }
        None
    }
}

pub struct McpBuilder {
    servers: Vec<McpServer>,
    factories: HashMap<String, ServerFactory>,
    mcp_channel_capacity: usize,
    roots: Vec<PathBuf>,
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    oauth_credential_store: Option<Arc<dyn OAuthCredentialStorage>>,
    aether_home: Option<PathBuf>,
    vars: Vars,
}

impl McpBuilder {
    pub fn new(workspace_root: impl AsRef<Path>) -> Self {
        Self {
            servers: Vec::new(),
            factories: HashMap::new(),
            mcp_channel_capacity: 1000,
            roots: Vec::new(),
            oauth_handler_factory: None,
            oauth_credential_store: None,
            aether_home: None,
            vars: Vars::new().with("WORKSPACE", workspace_root.as_ref().to_string_lossy().into_owned()),
        }
    }

    pub fn with_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.servers.extend(servers);
        self
    }

    pub fn register_in_memory_server(mut self, name: impl Into<String>, factory: ServerFactory) -> Self {
        self.factories.insert(name.into(), factory);
        self
    }

    pub fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.roots = roots;
        self
    }

    pub fn with_oauth_handler_factory(mut self, factory: OAuthHandlerFactory) -> Self {
        self.oauth_handler_factory = Some(factory);
        self
    }

    pub fn with_oauth_credential_store(mut self, store: Arc<dyn OAuthCredentialStorage>) -> Self {
        self.oauth_credential_store = Some(store);
        self
    }

    pub fn with_aether_home(mut self, aether_home: impl Into<PathBuf>) -> Self {
        self.aether_home = Some(aether_home.into());
        self
    }

    pub async fn from_json_files<T: AsRef<Path>>(mut self, paths: &[T]) -> Result<Self, ParseError> {
        if paths.is_empty() {
            return Ok(self);
        }
        let raw = McpConfig::from_json_files(paths)?;
        self.servers.extend(raw.into_servers(&self.factories, &self.vars).await?);
        Ok(self)
    }

    pub async fn from_mcp_config_sources(mut self, sources: &[McpConfigSource]) -> Result<Self, ParseError> {
        if sources.is_empty() {
            return Ok(self);
        }

        let mut merged = McpConfig::default();
        for source in sources {
            let config = match source {
                McpConfigSource::File { path, proxy } => {
                    let mut config = McpConfig::from_json_file(path)?;
                    if *proxy {
                        config.mark_all_proxy();
                    }
                    config
                }
                McpConfigSource::Json(json) => McpConfig::from_json(json)?,
                McpConfigSource::Inline(config) => config.clone(),
            };
            merged.servers.extend(config.servers);
        }

        self.servers.extend(merged.into_servers(&self.factories, &self.vars).await?);
        Ok(self)
    }

    pub async fn spawn(self) -> Result<McpSpawnResult, McpError> {
        let (mcp_command_tx, mcp_command_rx) = mpsc::channel::<McpCommand>(self.mcp_channel_capacity);
        let (event_tx, event_rx) = mpsc::channel::<McpClientEvent>(self.mcp_channel_capacity);

        let mut mcp_manager = McpManager::new(event_tx, self.oauth_handler_factory);
        if let Some(store) = self.oauth_credential_store {
            mcp_manager = mcp_manager.with_oauth_credential_store(store);
        }
        if let Some(aether_home) = self.aether_home {
            mcp_manager = mcp_manager.with_aether_home(aether_home);
        }

        if !self.roots.is_empty() {
            let roots = self.roots.into_iter().map(|path| root_from_path(&path, None)).collect();
            mcp_manager.set_roots(roots).await?;
        }

        mcp_manager.bootstrap_proxy_setup(&self.servers).await?;
        let pending = mcp_manager.register_pending(self.servers).await?;

        let mcp_handle = tokio::spawn(run_mcp_task(mcp_manager, mcp_command_rx, pending));

        Ok(McpSpawnResult { command_tx: mcp_command_tx, event_rx, handle: mcp_handle })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_utils::{
        client::{McpServerConfig, McpTransport, StdioServerConfig, StdioType},
        status::McpServerStatus,
    };
    use std::collections::{BTreeMap, HashMap};

    #[tokio::test]
    async fn mixed_direct_sources_preserve_last_wins_order() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mcp.json");
        std::fs::write(&file_path, r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#).unwrap();
        let inline = McpConfig::new(BTreeMap::from([(
            "coding".to_string(),
            McpServerConfig::Stdio(StdioServerConfig {
                type_: StdioType::Stdio,
                command: "from_inline".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                proxy: false,
            }),
        )]));
        let sources = vec![
            McpConfigSource::direct(file_path.clone()),
            McpConfigSource::Json(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#.to_string()),
            McpConfigSource::Inline(inline),
        ];

        let builder = McpBuilder::new("/workspace").from_mcp_config_sources(&sources).await.unwrap();

        assert_eq!(command_for(&builder, "coding"), Some("from_inline"));
        assert_eq!(proxy_for(&builder, "coding"), Some(false));
    }

    #[tokio::test]
    async fn file_sources_keep_their_position_relative_to_json_sources() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("mcp.json");
        std::fs::write(&file_path, r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#).unwrap();
        let sources = vec![
            McpConfigSource::Json(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#.to_string()),
            McpConfigSource::direct(file_path),
        ];

        let builder = McpBuilder::new("/workspace").from_mcp_config_sources(&sources).await.unwrap();

        assert_eq!(command_for(&builder, "coding"), Some("from_file"));
    }

    #[tokio::test]
    async fn file_source_proxy_true_marks_all_file_servers_proxied() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("proxied.json");
        std::fs::write(
            &file_path,
            r#"{"servers":{"github":{"type":"stdio","command":"g","proxy":false},"browser":{"type":"stdio","command":"b"}}}"#,
        )
        .unwrap();

        let builder = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[McpConfigSource::File { path: file_path, proxy: true }])
            .await
            .unwrap();

        assert_eq!(proxy_for(&builder, "github"), Some(true));
        assert_eq!(proxy_for(&builder, "browser"), Some(true));
    }

    #[tokio::test]
    async fn later_sources_override_proxy_flag() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("proxied.json");
        std::fs::write(&file_path, r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#).unwrap();
        let sources = vec![
            McpConfigSource::File { path: file_path, proxy: true },
            McpConfigSource::Json(
                r#"{"servers":{"coding":{"type":"stdio","command":"from_json","proxy":false}}}"#.to_string(),
            ),
        ];

        let builder = McpBuilder::new("/workspace").from_mcp_config_sources(&sources).await.unwrap();

        assert_eq!(command_for(&builder, "coding"), Some("from_json"));
        assert_eq!(proxy_for(&builder, "coding"), Some(false));
    }

    #[tokio::test]
    async fn spawn_returns_immediately_and_emits_initial_connecting_status() {
        let sources = vec![McpConfigSource::Json(
            r#"{"servers":{"slow":{"type":"stdio","command":"sleep","args":["30"]}}}"#.to_string(),
        )];

        let mut spawn = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&sources)
            .await
            .unwrap()
            .spawn()
            .await
            .expect("spawn should succeed");

        let event = spawn.event_rx.try_recv().expect("spawn() should buffer an initial ServerStatusesChanged");
        let McpClientEvent::ServerStatusesChanged(statuses) = event else {
            panic!("expected ServerStatusesChanged, got {event:?}");
        };
        assert!(matches!(statuses[0].status, McpServerStatus::Connecting));
        spawn.handle.abort();
    }

    #[tokio::test]
    async fn from_mcp_config_sources_expands_workspace_var_in_stdio_args() {
        let json = r#"{"servers":{"notes":{"type":"stdio","command":"server","args":["--dir","${WORKSPACE}/notes"]}}}"#;

        let builder =
            McpBuilder::new("/work").from_mcp_config_sources(&[McpConfigSource::Json(json.to_string())]).await.unwrap();

        assert_eq!(args_for(&builder, "notes"), Some(vec!["--dir".to_string(), "/work/notes".to_string()]));
    }

    fn command_for<'a>(builder: &'a McpBuilder, name: &str) -> Option<&'a str> {
        builder.servers.iter().find_map(|server| match &server.transport {
            McpTransport::Stdio { command, .. } if server.name == name => Some(command.as_str()),
            _ => None,
        })
    }

    fn args_for(builder: &McpBuilder, name: &str) -> Option<Vec<String>> {
        builder.servers.iter().find_map(|server| match &server.transport {
            McpTransport::Stdio { args, .. } if server.name == name => Some(args.clone()),
            _ => None,
        })
    }

    fn proxy_for(builder: &McpBuilder, name: &str) -> Option<bool> {
        builder.servers.iter().find(|server| server.name == name).map(|server| server.proxy)
    }
}
