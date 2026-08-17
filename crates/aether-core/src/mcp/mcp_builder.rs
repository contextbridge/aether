use mcp_utils::client::{
    McpClientEvent, McpConfig, McpConnectionDetails, McpError, McpManager, McpServer, OAuthHandlerFactory, ParseError,
    ProgressiveDiscoveryInstructions, ServerFactory, ToolFilter,
};
use mcp_utils::tool_gateway::UnixSocketPath;
use utils::{SettingsStore, variables::Vars};

use crate::agent_spec::McpConfigSource;
use crate::core::AgentDeps;
use crate::events::{AgentCommand, Command};

use super::run_mcp_task::{McpCommand, run_mcp_task};
use super::{DeferredToolGateway, DeferredToolGatewayHandle, McpCommandClient};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{self, Receiver, Sender},
        watch,
    },
    task::JoinHandle,
};

const CHANNEL_CAPACITY: usize = 1000;

type EnvironmentExtender = Arc<dyn Fn(Vec<(OsString, OsString)>) + Send + Sync>;

struct ProgressiveDiscovery {
    instructions: ProgressiveDiscoveryInstructions,
    extend_environment: EnvironmentExtender,
}

pub fn mcp(root_dir: impl AsRef<Path>) -> McpBuilder {
    McpBuilder::new(root_dir)
}

/// Owns a spawned MCP manager, deferred-tool gateway, and optional agent
/// synchronization task. Dropping this value shuts down the session.
pub struct McpRuntime {
    command_tx: Sender<McpCommand>,
    manager_handle: JoinHandle<()>,
    gateway_handle: Option<DeferredToolGatewayHandle>,
    agent_sync_handle: Option<JoinHandle<()>>,
    latest_snapshot: Option<watch::Receiver<McpConnectionDetails>>,
}

impl McpRuntime {
    pub fn command_client(&self) -> McpCommandClient {
        McpCommandClient::new(self.command_tx.clone())
    }

    pub fn has_deferred_tool_gateway(&self) -> bool {
        self.gateway_handle.as_ref().is_some_and(DeferredToolGatewayHandle::is_running)
    }

    pub fn deferred_tool_gateway_endpoint(&self) -> Option<&mcp_utils::tool_gateway::UnixSocketPath> {
        self.gateway_handle.as_ref().map(DeferredToolGatewayHandle::endpoint)
    }

    /// Returns the latest MCP state when this runtime was connected to an agent.
    pub fn latest_snapshot(&self) -> Option<McpConnectionDetails> {
        self.latest_snapshot.as_ref().map(|snapshot| snapshot.borrow().clone())
    }
}

impl Drop for McpRuntime {
    fn drop(&mut self) {
        self.manager_handle.abort();
        if let Some(handle) = &self.agent_sync_handle {
            handle.abort();
        }
    }
}

/// A fully assembled MCP session paired with its event stream.
pub struct McpSession {
    runtime: McpRuntime,
    event_rx: Receiver<McpClientEvent>,
}

impl McpSession {
    pub fn command_client(&self) -> McpCommandClient {
        self.runtime.command_client()
    }

    pub fn deferred_tool_gateway_endpoint(&self) -> Option<&UnixSocketPath> {
        self.runtime.deferred_tool_gateway_endpoint()
    }

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

    /// Connects MCP catalog events to an agent and returns only host-facing
    /// events such as statuses, authentication failures, and elicitation.
    pub fn connect_agent(
        mut self,
        agent_tx: Sender<Command>,
        initial_snapshot: McpConnectionDetails,
    ) -> (McpRuntime, Receiver<McpClientEvent>) {
        let (snapshot_tx, latest_snapshot) = watch::channel(initial_snapshot);
        let (host_event_tx, host_event_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let agent_sync_handle = tokio::spawn(synchronize_agent(self.event_rx, agent_tx, snapshot_tx, host_event_tx));
        self.runtime.agent_sync_handle = Some(agent_sync_handle);
        self.runtime.latest_snapshot = Some(latest_snapshot);
        (self.runtime, host_event_rx)
    }

    pub fn split(self) -> (McpRuntime, Receiver<McpClientEvent>) {
        (self.runtime, self.event_rx)
    }
}

pub struct McpBuilder {
    servers: Vec<McpServer>,
    factories: HashMap<String, ServerFactory>,
    root_dir: PathBuf,
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    agent_deps: AgentDeps,
    vars: Vars,
    tool_filter: ToolFilter,
    progressive_discovery: Option<ProgressiveDiscovery>,
}

impl McpBuilder {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        let mut vars = Vars::new().with("WORKSPACE", root_dir.as_ref().to_string_lossy().into_owned());

        if let Some(store) = SettingsStore::new("AETHER_HOME", ".aether") {
            vars.insert("AETHER_HOME", store.home().to_string_lossy().into_owned());
        }

        Self {
            servers: Vec::new(),
            factories: HashMap::new(),
            root_dir: root_dir.as_ref().to_path_buf(),
            oauth_handler_factory: None,
            agent_deps: AgentDeps::default(),
            vars,
            tool_filter: ToolFilter::default(),
            progressive_discovery: None,
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

    pub fn root_dir(&self) -> &Path {
        &self.root_dir
    }

    /// Cross-cutting dependencies handed to every agent spawned behind this
    /// builder's in-memory servers.
    pub fn agent_deps(&self) -> AgentDeps {
        self.agent_deps.clone()
    }

    pub fn with_agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    pub fn with_oauth_handler_factory(mut self, factory: OAuthHandlerFactory) -> Self {
        self.oauth_handler_factory = Some(factory);
        self
    }

    pub fn with_tool_filter(mut self, filter: ToolFilter) -> Self {
        self.tool_filter = filter;
        self
    }

    pub fn has_deferred_tools(&self) -> bool {
        self.servers.iter().any(McpServer::has_deferred_tools)
    }

    pub fn with_progressive_discovery(
        mut self,
        instructions: ProgressiveDiscoveryInstructions,
        extend_environment: impl Fn(Vec<(OsString, OsString)>) + Send + Sync + 'static,
    ) -> Self {
        self.progressive_discovery =
            Some(ProgressiveDiscovery { instructions, extend_environment: Arc::new(extend_environment) });
        self
    }

    pub fn with_aether_home(mut self, aether_home: impl Into<PathBuf>) -> Self {
        let aether_home = aether_home.into();
        self.vars.insert("AETHER_HOME", aether_home.to_string_lossy().into_owned());
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
                McpConfigSource::File { path, defer_tools } => {
                    let mut config = McpConfig::from_json_file(path)?;
                    if *defer_tools {
                        config.defer_all_tools();
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

    pub async fn spawn(self) -> Result<McpSession, McpError> {
        let progressive_discovery = if self.has_deferred_tools() {
            Some(self.progressive_discovery.ok_or_else(|| {
                McpError::Other("deferred tools require progressive discovery to be configured".to_string())
            })?)
        } else {
            None
        };
        let gateway = progressive_discovery.as_ref().map(|_| DeferredToolGateway::bind()).transpose()?;
        if let (Some(progressive), Some(gateway)) = (&progressive_discovery, &gateway) {
            (progressive.extend_environment)(gateway.environment());
        }

        let (command_tx, command_rx) = mpsc::channel(CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::channel::<McpClientEvent>(CHANNEL_CAPACITY);

        let mut mcp_manager = McpManager::new(event_tx, self.oauth_handler_factory).with_tool_filter(self.tool_filter);
        if let Some(progressive) = progressive_discovery {
            mcp_manager = mcp_manager.with_progressive_discovery_instructions(progressive.instructions);
        }
        if let Some(store) = self.agent_deps.oauth_credential_store {
            mcp_manager = mcp_manager.with_oauth_credential_store(store);
        }

        mcp_manager = mcp_manager.with_root_dir(self.root_dir);
        let pending = mcp_manager.register_pending(self.servers).await?;

        let manager_handle = tokio::spawn(run_mcp_task(mcp_manager, command_rx, pending));
        let gateway_handle = gateway.map(|gateway| gateway.start(McpCommandClient::new(command_tx.clone())));

        Ok(McpSession {
            runtime: McpRuntime {
                command_tx,
                manager_handle,
                gateway_handle,
                agent_sync_handle: None,
                latest_snapshot: None,
            },
            event_rx,
        })
    }
}

async fn synchronize_agent(
    mut event_rx: Receiver<McpClientEvent>,
    agent_tx: Sender<Command>,
    snapshot_tx: watch::Sender<McpConnectionDetails>,
    host_event_tx: Sender<McpClientEvent>,
) {
    while let Some(event) = event_rx.recv().await {
        let host_event = match event {
            McpClientEvent::ToolDefinitionsChanged(tool_definitions) => {
                snapshot_tx.send_modify(|snapshot| snapshot.tool_definitions.clone_from(&tool_definitions));
                send_agent_update(&agent_tx, AgentCommand::UpdateTools(tool_definitions)).await;
                None
            }
            McpClientEvent::ServerInstructionsUpdated { server, instructions } => {
                snapshot_tx.send_modify(|snapshot| match &instructions {
                    Some(body) => {
                        snapshot.instructions.insert(server.clone(), body.clone());
                    }
                    None => {
                        snapshot.instructions.remove(&server);
                    }
                });
                send_agent_update(&agent_tx, AgentCommand::UpdateMcpInstructions { server, body: instructions }).await;
                None
            }
            McpClientEvent::ServerStatusesChanged(server_statuses) => {
                snapshot_tx.send_modify(|snapshot| snapshot.server_statuses.clone_from(&server_statuses));
                Some(McpClientEvent::ServerStatusesChanged(server_statuses))
            }
            McpClientEvent::ConnectionReady(snapshot) => {
                snapshot_tx.send_replace(snapshot.clone());
                Some(McpClientEvent::ConnectionReady(snapshot))
            }
            event @ (McpClientEvent::Elicitation(_) | McpClientEvent::AuthenticationFailed { .. }) => Some(event),
        };

        if let Some(event) = host_event
            && !host_event_tx.is_closed()
        {
            let _ = host_event_tx.send(event).await;
        }
    }
}

async fn send_agent_update(agent_tx: &Sender<Command>, command: AgentCommand) {
    if let Err(error) = agent_tx.send(Command::agent(command)).await {
        tracing::debug!(%error, "Agent stopped before MCP catalog update could be applied");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_utils::{
        client::{McpServerConfig, McpTransport, StdioServerConfig, StdioType, ToolExposure},
        status::McpServerStatus,
    };
    use std::collections::{BTreeMap, HashMap};

    fn write_config_file(name: &str, json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, json).unwrap();
        (dir, path)
    }

    fn json_source(json: &str) -> McpConfigSource {
        McpConfigSource::Json(json.to_string())
    }

    async fn builder_from_sources(sources: &[McpConfigSource]) -> McpBuilder {
        McpBuilder::new("/workspace").from_mcp_config_sources(sources).await.unwrap()
    }

    #[tokio::test]
    async fn mixed_direct_sources_preserve_last_wins_order() {
        let (_dir, file_path) =
            write_config_file("mcp.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let inline = McpConfig::new(BTreeMap::from([(
            "coding".to_string(),
            McpServerConfig::Stdio(StdioServerConfig {
                type_: StdioType::Stdio,
                command: "from_inline".to_string(),
                args: Vec::new(),
                env: HashMap::new(),
                defer_tools: ToolExposure::ModelVisible,
            }),
        )]));
        let sources = vec![
            McpConfigSource::direct(file_path),
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#),
            McpConfigSource::Inline(inline),
        ];

        let builder = builder_from_sources(&sources).await;

        assert_eq!(command_for(&builder, "coding"), Some("from_inline"));
        assert_eq!(deferred_tools_for(&builder, "coding"), Some(false));
    }

    #[tokio::test]
    async fn file_sources_keep_their_position_relative_to_json_sources() {
        let (_dir, file_path) =
            write_config_file("mcp.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let sources = vec![
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#),
            McpConfigSource::direct(file_path),
        ];

        let builder = builder_from_sources(&sources).await;

        assert_eq!(command_for(&builder, "coding"), Some("from_file"));
    }

    #[tokio::test]
    async fn file_source_deferred_true_marks_all_file_servers_deferred() {
        let (_dir, file_path) = write_config_file(
            "deferred.json",
            r#"{"servers":{"github":{"type":"stdio","command":"g","deferTools":{"exclude":["status"]}},"browser":{"type":"stdio","command":"b"}}}"#,
        );

        let builder = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[McpConfigSource::deferred(file_path)])
            .await
            .unwrap();

        assert_eq!(deferred_tools_for(&builder, "github"), Some(true));
        assert_eq!(deferred_tools_for(&builder, "browser"), Some(true));
        assert!(is_model_visible_tool(&builder, "github", "status"));
    }

    #[tokio::test]
    async fn later_sources_override_deferred_flag() {
        let (_dir, file_path) =
            write_config_file("deferred.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let sources = vec![
            McpConfigSource::deferred(file_path),
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json","deferTools":false}}}"#),
        ];

        let builder = builder_from_sources(&sources).await;

        assert_eq!(command_for(&builder, "coding"), Some("from_json"));
        assert_eq!(deferred_tools_for(&builder, "coding"), Some(false));
    }

    #[tokio::test]
    async fn deferred_capability_is_required_only_by_effective_deferred_config() {
        let visible =
            builder_from_sources(&[json_source(r#"{"servers":{"coding":{"type":"stdio","command":"server"}}}"#)]).await;
        let deferred = builder_from_sources(&[json_source(
            r#"{"servers":{"github":{"type":"stdio","command":"server","deferTools":true}}}"#,
        )])
        .await;

        assert!(!visible.has_deferred_tools());
        assert!(deferred.has_deferred_tools());
    }

    #[tokio::test]
    async fn session_construction_rejects_unconfigured_deferred_tools() {
        let builder = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"deferred":{"type":"stdio","command":"server","deferTools":true}}}"#,
            )])
            .await
            .unwrap();

        let error = match builder.spawn().await {
            Ok(_) => panic!("deferred tools must not become inaccessible"),
            Err(error) => error,
        };

        assert!(matches!(error, McpError::Other(message) if message.contains("progressive discovery")));
    }

    #[tokio::test]
    async fn session_construction_starts_deferred_tool_gateway() {
        let session = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"deferred":{"type":"stdio","command":"server","deferTools":true}}}"#,
            )])
            .await
            .unwrap()
            .with_progressive_discovery(Arc::new(|_| String::new()), |_| {})
            .spawn()
            .await
            .unwrap();

        assert!(session.deferred_tool_gateway_endpoint().unwrap().socket_path().exists());
    }

    #[tokio::test]
    async fn spawn_returns_immediately_and_emits_initial_connecting_status() {
        let spawn = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"slow":{"type":"stdio","command":"sleep","args":["30"]}}}"#,
            )])
            .await
            .unwrap()
            .spawn()
            .await
            .expect("spawn should succeed");

        let (_runtime, mut event_rx) = spawn.split();
        let event = event_rx.try_recv().expect("spawn() should buffer an initial ServerStatusesChanged");
        let McpClientEvent::ServerStatusesChanged(statuses) = event else {
            panic!("expected ServerStatusesChanged, got {event:?}");
        };
        assert!(matches!(statuses[0].status, McpServerStatus::Connecting));
    }

    #[tokio::test]
    async fn from_mcp_config_sources_expands_workspace_var_in_stdio_args() {
        let builder = McpBuilder::new("/work")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"notes":{"type":"stdio","command":"server","args":["--dir","${WORKSPACE}/notes"]}}}"#,
            )])
            .await
            .unwrap();

        assert_eq!(args_for(&builder, "notes"), Some(vec!["--dir".to_string(), "/work/notes".to_string()]));
    }

    #[tokio::test]
    async fn from_mcp_config_sources_expands_aether_home_var_in_stdio_args() {
        let home = tempfile::tempdir().unwrap();

        let builder = McpBuilder::new("/work")
            .with_aether_home(home.path())
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"skills":{"type":"stdio","command":"server","args":["--dir","${AETHER_HOME}/skills"]}}}"#,
            )])
            .await
            .unwrap();

        assert_eq!(
            args_for(&builder, "skills"),
            Some(vec!["--dir".to_string(), home.path().join("skills").to_string_lossy().into_owned()])
        );
    }

    #[test]
    fn new_sets_root_directory_from_workspace_root() {
        let builder = McpBuilder::new("/workspace");
        assert_eq!(builder.root_dir, PathBuf::from("/workspace"));
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

    fn is_model_visible_tool(builder: &McpBuilder, server_name: &str, tool_name: &str) -> bool {
        builder
            .servers
            .iter()
            .find(|server| server.name == server_name)
            .is_some_and(|server| server.tool_exposure.is_model_visible(tool_name))
    }

    fn deferred_tools_for(builder: &McpBuilder, name: &str) -> Option<bool> {
        builder.servers.iter().find(|server| server.name == name).map(mcp_utils::client::McpServer::has_deferred_tools)
    }
}
