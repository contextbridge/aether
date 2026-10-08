use mcp_utils::McpError;
use mcp_utils::client::{ClientOptions, Elicitation, Transport};
use mcp_utils::config::{InMemoryServerConfig, McpConfig, McpServerConfig, ParseError};
use mcp_utils::gateway::{McpCatalog, McpGateway, ServerSpec, ToolExposure, ToolFilter};
use mcp_utils::model::Implementation;
use mcp_utils::server::{McpServer, ServerHandle};
use utils::{SettingsStore, variables::Vars};

use crate::agent_spec::McpConfigSource;
use crate::core::AgentDeps;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;
use thiserror::Error;
use tokio::sync::mpsc;

pub const AETHER_MCP_IPC_SOCKET: &str = "AETHER_MCP_IPC_SOCKET";

pub fn mcp(root_dir: impl AsRef<Path>) -> McpBuilder {
    McpBuilder::new(root_dir)
}

pub fn mcp_instructions(catalog: &McpCatalog) -> BTreeMap<String, String> {
    let mut instructions = catalog.instructions();
    if catalog.has_deferred_tools() {
        instructions
            .insert(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string(), PROGRESSIVE_DISCOVERY_INSTRUCTIONS.to_string());
    }
    instructions
}

#[derive(Clone)]
pub struct RuntimeServices {
    pub root_dir: PathBuf,
    pub agent_deps: AgentDeps,
    pub deferred_tools_socket: Option<PathBuf>,
}

pub type ServerFactory = Box<dyn Fn(InMemoryServerConfig, RuntimeServices) -> McpServer + Send + Sync>;

pub struct McpRuntime {
    gateway: McpGateway,
    deferred_tools: Option<ServerHandle>,
}

pub struct McpBuilder {
    servers: Vec<ConfiguredServer>,
    factories: HashMap<String, ServerFactory>,
    root_dir: PathBuf,
    agent_deps: AgentDeps,
    vars: Vars,
    tool_filter: ToolFilter,
    elicitations: Option<mpsc::Sender<Elicitation>>,
}

#[derive(Debug, Error)]
pub enum McpSpawnError {
    #[error(transparent)]
    Mcp(#[from] McpError),
    #[error("No factory is registered for in-memory MCP server '{0}'")]
    InMemoryFactoryNotFound(String),
    #[error("MCP server name '{0}' is reserved by Aether")]
    ReservedServerName(String),
}

impl McpRuntime {
    pub fn gateway(&self) -> &McpGateway {
        &self.gateway
    }

    pub fn deferred_tools_socket(&self) -> Option<&Path> {
        self.deferred_tools.as_ref().map(ServerHandle::path)
    }

    pub async fn shutdown(&mut self) {
        self.gateway.shutdown().await;
        self.deferred_tools.take();
    }
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
            agent_deps: AgentDeps::default(),
            vars,
            tool_filter: ToolFilter::default(),
            elicitations: None,
        }
    }

    pub fn with_config(mut self, config: McpConfig) -> Result<Self, ParseError> {
        for (name, server) in config.servers {
            self.servers.push(ConfiguredServer::from_config(name, server, &self.vars)?);
        }
        Ok(self)
    }

    pub fn with_servers(mut self, servers: Vec<ServerSpec>) -> Self {
        self.servers.extend(servers.into_iter().map(ConfiguredServer::Ready));
        self
    }

    pub fn from_mcp_config_sources(self, sources: &[McpConfigSource]) -> Result<Self, ParseError> {
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
        self.with_config(merged)
    }

    pub fn with_tool_filter(mut self, filter: ToolFilter) -> Self {
        self.tool_filter = filter;
        self
    }

    pub fn with_elicitations(mut self, sink: mpsc::Sender<Elicitation>) -> Self {
        self.elicitations = Some(sink);
        self
    }

    pub fn register_in_memory_server(
        mut self,
        name: impl Into<String>,
        factory: impl Fn(InMemoryServerConfig, RuntimeServices) -> McpServer + Send + Sync + 'static,
    ) -> Self {
        self.factories.insert(name.into(), Box::new(factory));
        self
    }

    pub fn with_agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    pub fn spawn(self) -> Result<McpRuntime, McpSpawnError> {
        let defers_tools = self.servers.iter().any(|server| server.exposure().has_deferred_tools());
        if defers_tools && self.servers.iter().any(|server| server.name() == PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME) {
            return Err(McpSpawnError::ReservedServerName(PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME.to_string()));
        }

        let gateway = McpGateway::new(self.client_options(), self.tool_filter);
        let deferred_tools = defers_tools
            .then(|| gateway.deferred_tools_server(Some(DEFERRED_TOOL_CALL_TIMEOUT)).serve_unix())
            .transpose()?;
        let services = RuntimeServices {
            root_dir: self.root_dir,
            agent_deps: self.agent_deps,
            deferred_tools_socket: deferred_tools.as_ref().map(|socket| socket.path().to_path_buf()),
        };
        let specs = self
            .servers
            .into_iter()
            .map(|server| server.build(&self.factories, &services))
            .collect::<Result<Vec<_>, _>>()?;
        gateway.add_servers(specs)?;

        Ok(McpRuntime { gateway, deferred_tools })
    }

    fn client_options(&self) -> ClientOptions {
        let mut options = ClientOptions::default()
            .implementation(Implementation::new("aether", env!("CARGO_PKG_VERSION")))
            .oauth_client_metadata_url(AETHER_OAUTH_CLIENT_METADATA_URL)
            .oauth_callback_port(AETHER_OAUTH_CALLBACK_PORT)
            .cwd(self.root_dir.clone());
        if let Some(store) = self.agent_deps.oauth_credential_store.clone() {
            options = options.oauth_store(store);
        }
        if let Some(sink) = self.elicitations.clone() {
            options = options.elicitation(sink);
        }
        if let Some(capability) = self.agent_deps.mcp_elicitation.clone() {
            options = options.elicitation_capability(capability);
        }
        options
    }
}

const DEFERRED_TOOL_CALL_TIMEOUT: Duration = Duration::from_mins(10);
const AETHER_OAUTH_CLIENT_METADATA_URL: &str = "https://aether-agent.io/oauth/client-metadata.json";
const AETHER_OAUTH_CALLBACK_PORT: u16 = 3118;
const PROGRESSIVE_DISCOVERY_INSTRUCTION_NAME: &str = "progressive-discovery";
const PROGRESSIVE_DISCOVERY_INSTRUCTIONS: &str = include_str!("progressive_discovery_instructions.md");

enum ConfiguredServer {
    Ready(ServerSpec),
    InMemory { name: String, config: InMemoryServerConfig },
}

impl ConfiguredServer {
    fn from_config(name: String, config: McpServerConfig, vars: &Vars) -> Result<Self, ParseError> {
        let exposure = config.defer_tools().clone();
        let transport = match config {
            McpServerConfig::Stdio(config) => config.into_transport(vars)?,
            McpServerConfig::Remote(config) => config.into_transport(vars)?,
            McpServerConfig::InMemory(config) => return Ok(Self::InMemory { name, config: config.expand(vars)? }),
        };
        Ok(Self::Ready(ServerSpec { name, transport, exposure }))
    }

    fn name(&self) -> &str {
        match self {
            Self::Ready(spec) => &spec.name,
            Self::InMemory { name, .. } => name,
        }
    }

    fn exposure(&self) -> &ToolExposure {
        match self {
            Self::Ready(spec) => &spec.exposure,
            Self::InMemory { config, .. } => &config.defer_tools,
        }
    }

    fn build(
        self,
        factories: &HashMap<String, ServerFactory>,
        services: &RuntimeServices,
    ) -> Result<ServerSpec, McpSpawnError> {
        let (name, config) = match self {
            Self::Ready(spec) => return Ok(spec),
            Self::InMemory { name, config } => (name, config),
        };
        let factory = factories.get(&name).ok_or_else(|| McpSpawnError::InMemoryFactoryNotFound(name.clone()))?;
        let exposure = config.defer_tools.clone();
        let server = factory(config, services.clone());
        Ok(ServerSpec { name, transport: Transport::InProcess(server), exposure })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_auth::{FakeOAuthCredentialStore, OAuthCredentialStorage};
    use mcp_utils::config::{StdioServerConfig, StdioType};
    use mcp_utils::testing::{FakeMcpServer, fake_mcp};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use utils::mcp_status::{McpServerStatus, McpServerStatusEntry};

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
        let factory: ServerFactory = Box::new(move |config, services| {
            factory_calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(config.args, ["--root", "/workspace/tools"]);
            *factory_received.lock().unwrap() = Some(services);
            FakeMcpServer::new().into()
        });

        let builder = McpBuilder::new("/workspace")
            .with_agent_deps(deps)
            .register_in_memory_server("test", factory)
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"test":{"type":"in-memory","args":["--root","${WORKSPACE}/tools"]}}}"#,
            )])
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 0);

        let _spawned = builder.spawn().unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let services = received.lock().unwrap().clone().expect("factory received runtime services");
        assert_eq!(services.root_dir, PathBuf::from("/workspace"));
        assert!(Arc::ptr_eq(
            services.agent_deps.oauth_credential_store.as_ref().expect("factory received agent dependencies"),
            &oauth_store,
        ));
        assert!(services.deferred_tools_socket.is_none());
    }

    #[tokio::test]
    async fn deferred_gateway_is_bound_before_in_memory_factories_run() {
        let received = Arc::new(Mutex::new(None::<RuntimeServices>));
        let factory_received = Arc::clone(&received);
        let factory: ServerFactory = Box::new(move |_, services| {
            *factory_received.lock().unwrap() = Some(services);
            FakeMcpServer::new().into()
        });
        let runtime = McpBuilder::new("/workspace")
            .register_in_memory_server("test", factory)
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"test":{"type":"in-memory","deferTools":true}}}"#)])
            .unwrap()
            .spawn()
            .unwrap();

        let services = received.lock().unwrap().clone().expect("factory received runtime services");
        let socket = services.deferred_tools_socket.expect("factory receives gateway endpoint");
        assert_eq!(socket, runtime.deferred_tools_socket().expect("gateway endpoint exists"));
        assert!(socket.exists());
    }

    #[tokio::test]
    async fn spawned_in_memory_servers_connect_in_the_background() {
        let factory: ServerFactory = Box::new(|_, _| FakeMcpServer::new().into());
        let runtime = McpBuilder::new("/workspace")
            .register_in_memory_server("test", factory)
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"test":{"type":"in-memory"}}}"#)])
            .unwrap()
            .spawn()
            .unwrap();

        let ready = runtime.gateway().ready().await;

        assert_eq!(ready.tools()[0].name, "test__add_numbers");
    }

    #[tokio::test]
    async fn ready_servers_and_config_servers_are_added_in_order() {
        let factory: ServerFactory = Box::new(|_, _| FakeMcpServer::new().into());
        let runtime = McpBuilder::new("/workspace")
            .register_in_memory_server("configured", factory)
            .with_servers(vec![fake_mcp("ready", FakeMcpServer::new())])
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"configured":{"type":"in-memory"}}}"#)])
            .unwrap()
            .spawn()
            .unwrap();

        let statuses = runtime.gateway().ready().await.statuses();

        assert_eq!(statuses.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), ["ready", "configured"]);
    }

    #[tokio::test]
    async fn missing_in_memory_factory_fails_at_spawn_with_server_name() {
        let builder = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(r#"{"servers":{"custom":{"type":"in-memory"}}}"#)])
            .unwrap();

        let Err(error) = builder.spawn() else {
            panic!("spawn should reject an unregistered factory");
        };
        assert!(matches!(error, McpSpawnError::InMemoryFactoryNotFound(ref server) if server == "custom"));
    }

    #[test]
    fn invalid_header_fails_when_config_is_added() {
        let result = McpBuilder::new("/workspace").from_mcp_config_sources(&[json_source(
            r#"{"servers":{"remote":{"type":"http","url":"https://example.com","headers":{"X-Key":"bad\nvalue"}}}}"#,
        )]);

        assert!(matches!(result, Err(ParseError::InvalidHeaderValue { .. })));
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
            McpConfigSource::model_visible(file_path),
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#),
            McpConfigSource::Inline(inline),
        ];

        let coding = only_status(builder_from_sources(&sources)).await;

        assert_eq!(spawned_command(&coding), "from_inline");
        assert!(!coding.deferred_tools);
    }

    #[tokio::test]
    async fn file_sources_keep_their_position_relative_to_json_sources() {
        let (_dir, file_path) =
            write_config_file("mcp.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let sources = vec![
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json"}}}"#),
            McpConfigSource::model_visible(file_path),
        ];

        let coding = only_status(builder_from_sources(&sources)).await;

        assert_eq!(spawned_command(&coding), "from_file");
    }

    #[tokio::test]
    async fn file_source_defer_tools_marks_all_file_servers_deferred() {
        let (_dir, file_path) = write_config_file(
            "deferred.json",
            r#"{"servers":{"github":{"type":"in-memory","deferTools":{"exclude":["add_numbers"]}},"browser":{"type":"stdio","command":"b"}}}"#,
        );
        let factory: ServerFactory = Box::new(|_, _| FakeMcpServer::new().into());

        let runtime = McpBuilder::new("/workspace")
            .register_in_memory_server("github", factory)
            .from_mcp_config_sources(&[McpConfigSource::File { path: file_path, defer_tools: true }])
            .unwrap()
            .spawn()
            .unwrap();
        let catalog = runtime.gateway().ready().await;

        assert!(catalog.statuses().iter().all(|status| status.deferred_tools));
        let tools = catalog.tools().into_iter().map(|tool| tool.name.to_string()).collect::<Vec<_>>();
        assert_eq!(tools, ["github__add_numbers"]);
    }

    #[tokio::test]
    async fn later_sources_override_defer_tools_flag() {
        let (_dir, file_path) =
            write_config_file("deferred.json", r#"{"servers":{"coding":{"type":"stdio","command":"from_file"}}}"#);
        let sources = vec![
            McpConfigSource::File { path: file_path, defer_tools: true },
            json_source(r#"{"servers":{"coding":{"type":"stdio","command":"from_json","deferTools":false}}}"#),
        ];

        let coding = only_status(builder_from_sources(&sources)).await;

        assert_eq!(spawned_command(&coding), "from_json");
        assert!(!coding.deferred_tools);
    }

    #[tokio::test]
    async fn spawn_returns_before_servers_connect() {
        let runtime = McpBuilder::new(std::env::temp_dir())
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"slow":{"type":"stdio","command":"sleep","args":["30"]}}}"#,
            )])
            .unwrap()
            .spawn()
            .expect("spawn should succeed");

        let statuses = runtime.gateway().catalog().statuses();
        assert!(matches!(statuses[0].status, McpServerStatus::Connecting));
    }

    #[tokio::test]
    async fn from_mcp_config_sources_expands_workspace_var_in_args() {
        let args = spawned_args(
            "/work",
            r#"{"servers":{"notes":{"type":"in-memory","args":["--dir","${WORKSPACE}/notes"]}}}"#,
        );

        assert_eq!(args, ["--dir", "/work/notes"]);
    }

    #[tokio::test]
    async fn from_mcp_config_sources_expands_aether_home_var_in_args() {
        let home = SettingsStore::new("AETHER_HOME", ".aether").expect("Aether home resolves").home().to_path_buf();

        let args = spawned_args(
            "/work",
            r#"{"servers":{"notes":{"type":"in-memory","args":["--dir","${AETHER_HOME}/skills"]}}}"#,
        );

        assert_eq!(args, ["--dir".to_string(), home.join("skills").to_string_lossy().into_owned()]);
    }

    #[tokio::test]
    async fn reserved_progressive_discovery_server_is_rejected_when_a_server_defers_tools() {
        let result = McpBuilder::new("/workspace")
            .from_mcp_config_sources(&[json_source(
                r#"{"servers":{"progressive-discovery":{"type":"stdio","command":"server"},"deferred":{"type":"stdio","command":"server","deferTools":true}}}"#,
            )])
            .unwrap()
            .spawn();

        assert!(matches!(
            result,
            Err(McpSpawnError::ReservedServerName(name)) if name == "progressive-discovery"
        ));
    }

    async fn only_status(builder: McpBuilder) -> McpServerStatusEntry {
        let runtime = builder.spawn().unwrap();
        let mut statuses = runtime.gateway().ready().await.statuses();
        assert_eq!(statuses.len(), 1);
        statuses.remove(0)
    }

    fn spawned_command(status: &McpServerStatusEntry) -> &str {
        let McpServerStatus::Failed { error } = &status.status else { panic!("expected a failed spawn: {status:?}") };
        error.strip_prefix("Failed to spawn '").and_then(|rest| rest.split_once('\'')).expect("spawn error").0
    }

    fn spawned_args(root_dir: &str, json: &str) -> Vec<String> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let factory_received = Arc::clone(&received);
        let factory: ServerFactory = Box::new(move |config, _| {
            *factory_received.lock().unwrap() = config.args;
            FakeMcpServer::new().into()
        });
        McpBuilder::new(root_dir)
            .register_in_memory_server("notes", factory)
            .from_mcp_config_sources(&[json_source(json)])
            .unwrap()
            .spawn()
            .unwrap();
        received.lock().unwrap().clone()
    }
}
