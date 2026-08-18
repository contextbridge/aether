use mcp_utils::client::{
    InMemoryServerSpec, McpClientEvent, McpConfig, McpConnectionDetails, McpError, McpManager, McpServer, McpTransport,
    OAuthHandlerFactory, PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME, ParseError, RuntimeMcpServer, RuntimeMcpTransport,
    ToolFilter,
};
use mcp_utils::tool_gateway::{AETHER_MCP_IPC_SOCKET, UnixSocketMcpTransport, UnixSocketPath, UnixSocketServer};
use utils::{SettingsStore, variables::Vars};

use crate::agent_spec::McpConfigSource;
use crate::core::AgentDeps;
use crate::events::{AgentCommand, Command};

use super::{
    gateway_service::GatewayService,
    mcp_handle::McpHandle,
    run_mcp_task::{ManagerCommand, run_mcp_task},
};
use futures::future::BoxFuture;
use rmcp::{RoleServer, service::DynService};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::{
    sync::{
        mpsc::{self, Receiver},
        watch,
    },
    task::JoinHandle,
};

pub fn mcp(root_dir: impl AsRef<Path>) -> McpBuilder {
    McpBuilder::new(root_dir)
}

#[derive(Clone)]
pub struct RuntimeServices {
    pub mcp: McpHandle,
    pub root_dir: PathBuf,
    pub agent_deps: AgentDeps,
    pub shell_environment: BTreeMap<String, String>,
}

pub type ServerFactory = Box<
    dyn Fn(InMemoryServerSpec, RuntimeServices) -> BoxFuture<'static, Box<dyn DynService<RoleServer>>> + Send + Sync,
>;

/// Owns a spawned MCP manager. Dropping this value aborts the manager task.
pub struct McpRuntime {
    mcp: McpHandle,
    handle: JoinHandle<()>,
    agent_sync_handle: Option<JoinHandle<()>>,
    gateway: Option<UnixSocketServer>,
}

impl McpRuntime {
    pub fn handle(&self) -> &McpHandle {
        &self.mcp
    }

    pub fn gateway_endpoint(&self) -> Option<&Path> {
        self.gateway.as_ref().map(UnixSocketServer::path)
    }
}

impl Drop for McpRuntime {
    fn drop(&mut self) {
        self.handle.abort();
        if let Some(handle) = &self.agent_sync_handle {
            handle.abort();
        }
    }
}

/// A freshly spawned MCP manager paired with its event stream. Consumers
/// receive incremental updates over the event stream (starting with an initial
/// `ServerStatusesChanged` reflecting every configured server in `Connecting`)
/// and can call [`split`](Self::split) to separate the stream from the
/// [`McpRuntime`] that keeps the manager task alive.
pub struct McpSession {
    runtime: McpRuntime,
    event_rx: Receiver<McpClientEvent>,
}

impl McpSession {
    pub fn handle(&self) -> &McpHandle {
        self.runtime.handle()
    }

    pub fn gateway_endpoint(&self) -> Option<&Path> {
        self.runtime.gateway_endpoint()
    }

    /// Synchronize this session's current and future tools and instructions with
    /// one agent. Initial state is sent before this method returns.
    pub async fn connect_agent(mut self, agent_tx: mpsc::Sender<Command>) -> Self {
        assert!(self.runtime.agent_sync_handle.is_none(), "an MCP session can only connect one agent");
        let mut snapshots = self.runtime.handle().subscribe();
        let initial = snapshots.borrow_and_update().clone();
        let mut previous_tools = initial.tool_definitions();
        let mut previous_instructions = initial.model_instructions();
        if agent_tx.send(Command::agent(AgentCommand::UpdateTools(previous_tools.clone()))).await.is_err() {
            return self;
        }
        for (server, body) in &previous_instructions {
            if agent_tx
                .send(Command::agent(AgentCommand::UpdateMcpInstructions {
                    server: server.clone(),
                    body: Some(body.clone()),
                }))
                .await
                .is_err()
            {
                return self;
            }
        }

        self.runtime.agent_sync_handle = Some(tokio::spawn(async move {
            while snapshots.changed().await.is_ok() {
                let snapshot = snapshots.borrow_and_update().clone();
                let tools = snapshot.tool_definitions();
                if tools != previous_tools {
                    if agent_tx.send(Command::agent(AgentCommand::UpdateTools(tools.clone()))).await.is_err() {
                        break;
                    }
                    previous_tools = tools;
                }

                let instructions = snapshot.model_instructions();
                let servers = previous_instructions.keys().chain(instructions.keys()).cloned().collect::<BTreeSet<_>>();
                for server in servers {
                    let previous = previous_instructions.get(&server);
                    let next = instructions.get(&server);
                    if previous != next
                        && agent_tx
                            .send(Command::agent(AgentCommand::UpdateMcpInstructions { server, body: next.cloned() }))
                            .await
                            .is_err()
                    {
                        return;
                    }
                }
                previous_instructions = instructions;
            }
        }));
        self
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

    pub fn split(self) -> (McpRuntime, Receiver<McpClientEvent>) {
        (self.runtime, self.event_rx)
    }
}

pub struct McpBuilder {
    servers: Vec<McpServer>,
    factories: HashMap<String, ServerFactory>,
    mcp_channel_capacity: usize,
    root_dir: PathBuf,
    oauth_handler_factory: Option<OAuthHandlerFactory>,
    agent_deps: AgentDeps,
    aether_home: Option<PathBuf>,
    vars: Vars,
    tool_filter: ToolFilter,
    progressive_discovery_instructions: Option<String>,
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
            mcp_channel_capacity: 1000,
            root_dir: root_dir.as_ref().to_path_buf(),
            oauth_handler_factory: None,
            agent_deps: AgentDeps::default(),
            aether_home: None,
            vars,
            tool_filter: ToolFilter::default(),
            progressive_discovery_instructions: None,
        }
    }

    pub fn with_servers(mut self, servers: Vec<McpServer>) -> Self {
        self.servers.extend(servers);
        self
    }

    pub fn with_tool_filter(mut self, filter: ToolFilter) -> Self {
        self.tool_filter = filter;
        self
    }

    pub fn with_progressive_discovery_instructions(mut self, instructions: impl Into<String>) -> Self {
        self.progressive_discovery_instructions = Some(instructions.into());
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

    pub fn with_aether_home(mut self, aether_home: impl Into<PathBuf>) -> Self {
        let aether_home = aether_home.into();
        self.vars.insert("AETHER_HOME", aether_home.to_string_lossy().into_owned());
        self.aether_home = Some(aether_home);
        self
    }

    pub fn from_json_files<T: AsRef<Path>>(mut self, paths: &[T]) -> Result<Self, ParseError> {
        if paths.is_empty() {
            return Ok(self);
        }
        let raw = McpConfig::from_json_files(paths)?;
        self.servers.extend(raw.into_servers(&self.vars)?);
        Ok(self)
    }

    pub fn from_mcp_config_sources(mut self, sources: &[McpConfigSource]) -> Result<Self, ParseError> {
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

        self.servers.extend(merged.into_servers(&self.vars)?);
        Ok(self)
    }

    pub async fn spawn(self) -> Result<McpSession, McpError> {
        let McpBuilder {
            servers,
            factories,
            mcp_channel_capacity,
            root_dir,
            oauth_handler_factory,
            agent_deps,
            aether_home,
            vars: _,
            tool_filter,
            progressive_discovery_instructions,
        } = self;
        if servers.iter().any(|server| server.tool_exposure.is_proxied())
            && servers.iter().any(|server| server.name == PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME)
        {
            return Err(McpError::ReservedServerName(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string()));
        }
        let (manager_tx, manager_rx) = mpsc::channel::<ManagerCommand>(mcp_channel_capacity);
        let (snapshot_tx, snapshot_rx) = watch::channel(Arc::new(mcp_utils::client::McpSnapshot::default()));
        let (event_tx, event_rx) = mpsc::channel::<McpClientEvent>(mcp_channel_capacity);
        let mcp = McpHandle::new(manager_tx, snapshot_rx);
        let gateway_transport = if servers.iter().any(|server| server.tool_exposure.is_proxied()) {
            let path = UnixSocketPath::new().map_err(|error| McpError::TransportError(error.to_string()))?;
            Some(UnixSocketMcpTransport::bind(path).map_err(|error| McpError::TransportError(error.to_string()))?)
        } else {
            None
        };
        let shell_environment = gateway_transport
            .as_ref()
            .map(|transport| {
                BTreeMap::from([(AETHER_MCP_IPC_SOCKET.to_string(), transport.path().to_string_lossy().into_owned())])
            })
            .unwrap_or_default();
        let services = RuntimeServices { mcp: mcp.clone(), root_dir: root_dir.clone(), agent_deps, shell_environment };
        let servers = resolve_servers(servers, &factories, &services).await?;

        let mut mcp_manager = McpManager::new(event_tx, oauth_handler_factory)
            .with_tool_filter(tool_filter)
            .with_snapshot_sender(snapshot_tx);
        if let Some(instructions) = progressive_discovery_instructions {
            mcp_manager = mcp_manager.with_progressive_discovery_instructions(instructions);
        }
        if let Some(store) = services.agent_deps.oauth_credential_store.clone() {
            mcp_manager = mcp_manager.with_oauth_credential_store(store);
        }
        if let Some(aether_home) = aether_home {
            mcp_manager = mcp_manager.with_aether_home(aether_home);
        }
        mcp_manager = mcp_manager.with_root_dir(root_dir);
        let pending = mcp_manager.register_pending(servers).await?;
        let task = tokio::spawn(run_mcp_task(mcp_manager, manager_rx, pending));
        let gateway = gateway_transport.map(|transport| transport.spawn(GatewayService::new(mcp.clone())));

        Ok(McpSession { runtime: McpRuntime { mcp, handle: task, agent_sync_handle: None, gateway }, event_rx })
    }
}

async fn resolve_servers(
    servers: Vec<McpServer>,
    factories: &HashMap<String, ServerFactory>,
    services: &RuntimeServices,
) -> Result<Vec<RuntimeMcpServer>, McpError> {
    let mut resolved = Vec::with_capacity(servers.len());
    for McpServer { name, transport, tool_exposure } in servers {
        let transport = match transport {
            McpTransport::Stdio { command, args, env } => RuntimeMcpTransport::Stdio { command, args, env },
            McpTransport::Http(config) => RuntimeMcpTransport::Http(config),
            McpTransport::InMemory { spec } => {
                let factory = factories.get(&spec.factory).ok_or_else(|| McpError::InMemoryFactoryNotFound {
                    server: name.clone(),
                    factory: spec.factory.clone(),
                })?;
                RuntimeMcpTransport::InMemory { server: factory(spec, services.clone()).await }
            }
        };
        resolved.push(RuntimeMcpServer::new(name, transport, tool_exposure));
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_auth::{FakeOAuthCredentialStore, OAuthCredentialStorage};
    use futures::FutureExt;
    use mcp_utils::testing::FakeMcpServer;
    use mcp_utils::{
        client::{McpServerConfig, McpTransport, StdioServerConfig, StdioType, ToolExposure},
        status::McpServerStatus,
    };
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    fn write_config_file(name: &str, json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, json).unwrap();
        (dir, path)
    }

    fn json_source(json: &str) -> McpConfigSource {
        McpConfigSource::Json(json.to_string())
    }

    fn builder_from_sources(sources: &[McpConfigSource]) -> McpBuilder {
        McpBuilder::new("/workspace").from_mcp_config_sources(sources).unwrap()
    }

    #[tokio::test]
    async fn in_memory_factory_runs_once_at_spawn_with_runtime_services() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(None::<RuntimeServices>));
        let factory_calls = Arc::clone(&calls);
        let factory_received = Arc::clone(&received);
        let oauth_store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        let deps = AgentDeps::new(Arc::clone(&oauth_store), None);
        let factory: ServerFactory = Box::new(move |spec, services| {
            factory_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(spec.args, ["--root", "/workspace/tools"]);
            assert_eq!(spec.input, Some(serde_json::json!({"enabled": true})));
            *factory_received.lock().unwrap() = Some(services);
            async move { FakeMcpServer::new().into_dyn() }.boxed()
        });

        let builder = McpBuilder::new("/workspace")
            .with_agent_deps(deps)
            .register_in_memory_server("test", factory)
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"test":{"type":"in-memory","args":["--root","${WORKSPACE}/tools"],"input":{"enabled":true}}}}"#,
            )])
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let spawn = builder.spawn().await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let services = received.lock().unwrap().clone().expect("factory received runtime services");
        assert_eq!(services.root_dir, PathBuf::from("/workspace"));
        assert!(Arc::ptr_eq(&services.mcp.snapshot(), &spawn.handle().snapshot()));
        assert!(Arc::ptr_eq(
            services.agent_deps.oauth_credential_store.as_ref().expect("factory received agent dependencies"),
            &oauth_store,
        ));
        assert!(services.shell_environment.is_empty());
    }

    #[tokio::test]
    async fn deferred_gateway_is_bound_before_in_memory_factories_run() {
        let received = Arc::new(Mutex::new(None::<RuntimeServices>));
        let factory_received = Arc::clone(&received);
        let factory: ServerFactory = Box::new(move |_, services| {
            *factory_received.lock().unwrap() = Some(services);
            async move { FakeMcpServer::new().into_dyn() }.boxed()
        });
        let spawn = McpBuilder::new("/workspace")
            .register_in_memory_server("test", factory)
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"test":{"type":"in-memory","proxy":true}}}"#)])
            .unwrap()
            .spawn()
            .await
            .unwrap();

        let services = received.lock().unwrap().clone().expect("factory received runtime services");
        let inherited =
            services.shell_environment.get(AETHER_MCP_IPC_SOCKET).expect("factory receives gateway endpoint");
        assert_eq!(Path::new(inherited), spawn.gateway_endpoint().expect("gateway endpoint exists"));
        assert!(Path::new(inherited).exists());
    }

    #[tokio::test]
    async fn snapshots_are_immutable_and_watch_observes_connection_changes() {
        let factory: ServerFactory = Box::new(|_, _| async move { FakeMcpServer::new().into_dyn() }.boxed());
        let mut spawn = McpBuilder::new("/workspace")
            .register_in_memory_server("test", factory)
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"test":{"type":"in-memory"}}}"#)])
            .unwrap()
            .spawn()
            .await
            .unwrap();
        let old = spawn.handle().snapshot();
        let mut updates = spawn.handle().subscribe();

        let ready = spawn.block_until_ready().await.expect("bootstrap completes");
        updates.changed().await.expect("connection publishes a snapshot");
        let observed = updates.borrow().clone();

        assert!(old.tool_definitions().is_empty());
        assert_eq!(ready.tool_definitions()[0].name, "test__add_numbers");
        assert_eq!(observed.tool_definitions(), ready.tool_definitions());
        assert!(!Arc::ptr_eq(&old, &ready));
    }

    #[tokio::test]
    async fn missing_in_memory_factory_fails_at_spawn_with_server_and_factory() {
        let builder = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"custom":{"type":"in-memory"}}}"#)])
            .unwrap();

        let Err(error) = builder.spawn().await else {
            panic!("spawn should reject an unregistered factory");
        };
        assert!(matches!(
            error,
            McpError::InMemoryFactoryNotFound { ref server, ref factory }
                if server == "custom" && factory == "custom"
        ));
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
                proxy: ToolExposure::Direct,
            }),
        )]));
        let sources = vec![
            McpConfigSource::direct(file_path),
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#),
            McpConfigSource::Inline(inline),
        ];

        let builder = builder_from_sources(&sources);

        assert_eq!(command_for(&builder, "coding"), Some("from_inline"));
        assert_eq!(proxy_for(&builder, "coding"), Some(false));
    }

    #[tokio::test]
    async fn file_sources_keep_their_position_relative_to_json_sources() {
        let (_dir, file_path) =
            write_config_file("mcp.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let sources = vec![
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#),
            McpConfigSource::direct(file_path),
        ];

        let builder = builder_from_sources(&sources);

        assert_eq!(command_for(&builder, "coding"), Some("from_file"));
    }

    #[tokio::test]
    async fn file_source_proxy_true_marks_all_file_servers_proxied() {
        let (_dir, file_path) = write_config_file(
            "proxied.json",
            r#"{"servers":{"github":{"type":"stdio","command":"g","proxy":{"exclude":["status"]}},"browser":{"type":"stdio","command":"b"}}}"#,
        );

        let builder = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[McpConfigSource::File { path: file_path, proxy: true }])
            .unwrap();

        assert_eq!(proxy_for(&builder, "github"), Some(true));
        assert_eq!(proxy_for(&builder, "browser"), Some(true));
        assert!(is_direct_tool(&builder, "github", "status"));
    }

    #[tokio::test]
    async fn later_sources_override_proxy_flag() {
        let (_dir, file_path) =
            write_config_file("proxied.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let sources = vec![
            McpConfigSource::File { path: file_path, proxy: true },
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json","proxy":false}}}"#),
        ];

        let builder = builder_from_sources(&sources);

        assert_eq!(command_for(&builder, "coding"), Some("from_json"));
        assert_eq!(proxy_for(&builder, "coding"), Some(false));
    }

    #[tokio::test]
    async fn spawn_returns_immediately_and_emits_initial_connecting_status() {
        let spawn = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"slow":{"type":"stdio","command":"sleep","args":["30"]}}}"#,
            )])
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
            .unwrap();

        assert_eq!(
            args_for(&builder, "skills"),
            Some(vec!["--dir".to_string(), home.path().join("skills").to_string_lossy().into_owned()])
        );
    }

    #[tokio::test]
    async fn reserved_progressive_discovery_server_is_rejected_when_gateway_is_enabled() {
        let result = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"progressive-discovery":{"type":"stdio","command":"server"},"deferred":{"type":"stdio","command":"server","proxy":true}}}"#,
            )])
            .unwrap()
            .spawn()
            .await;

        assert!(matches!(result, Err(McpError::ReservedServerName(name)) if name == "progressive-discovery"));
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

    fn is_direct_tool(builder: &McpBuilder, server_name: &str, tool_name: &str) -> bool {
        builder
            .servers
            .iter()
            .find(|server| server.name == server_name)
            .is_some_and(|server| server.tool_exposure.is_direct_tool(tool_name))
    }

    fn proxy_for(builder: &McpBuilder, name: &str) -> Option<bool> {
        builder.servers.iter().find(|server| server.name == name).map(mcp_utils::client::McpServer::is_proxied)
    }
}
