use mcp_utils::client::{ClientOptions, ElicitationSink, Transport};
use mcp_utils::gateway::{McpGateway, ServerSpec};
use mcp_utils::model::Implementation;
use mcp_utils::server::{McpServer, ServeError, UnixSocket};
use utils::settings::aether_home;
use utils::variables::Vars;

use super::config::ServerSource;
use super::{McpCatalog, McpConfig, McpServerConfig, ParseError, ToolExposure, ToolFilter};
use crate::core::AgentDeps;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub fn mcp(root_dir: impl AsRef<Path>) -> McpBuilder {
    McpBuilder::new(root_dir)
}

#[derive(Clone)]
pub struct RuntimeServices {
    pub root_dir: PathBuf,
    pub agent_deps: AgentDeps,
    pub deferred_tools_socket: Option<PathBuf>,
}

pub struct McpBuilder {
    servers: Vec<ConfiguredServer>,
    factories: HashMap<String, ServerFactory>,
    root_dir: PathBuf,
    agent_deps: AgentDeps,
    vars: Vars,
    tool_filter: ToolFilter,
    elicitation: Option<ElicitationSink>,
}

pub type ServerFactoryError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Error)]
pub enum McpSpawnError {
    #[error(transparent)]
    Serve(#[from] ServeError),
    #[error("No factory is registered for in-memory MCP server '{0}'")]
    InMemoryFactoryNotFound(String),
    #[error("Failed to create in-memory MCP server '{name}': {source}")]
    ServerFactory { name: String, source: ServerFactoryError },
}

impl McpBuilder {
    pub fn new(root_dir: impl AsRef<Path>) -> Self {
        let mut vars = Vars::new().with("WORKSPACE", root_dir.as_ref().to_string_lossy().into_owned());

        if let Some(home) = aether_home() {
            vars.insert("AETHER_HOME", home.to_string_lossy().into_owned());
        }

        Self {
            servers: Vec::new(),
            factories: HashMap::new(),
            root_dir: root_dir.as_ref().to_path_buf(),
            agent_deps: AgentDeps::default(),
            vars,
            tool_filter: ToolFilter::default(),
            elicitation: None,
        }
    }

    pub fn with_config(mut self, config: McpConfig) -> Result<Self, ParseError> {
        for (name, server) in config.servers {
            let server = ConfiguredServer::from_config(name, server, &self.vars, &self.root_dir)?;
            self.insert(server);
        }
        Ok(self)
    }

    pub fn with_servers(self, servers: Vec<ServerSpec>) -> Self {
        servers.into_iter().fold(self, |builder, spec| builder.with_server(spec, ToolExposure::ModelVisible))
    }

    pub fn with_server(mut self, spec: ServerSpec, exposure: ToolExposure) -> Self {
        self.insert(ConfiguredServer { name: spec.name, exposure, source: ServerSource::Ready(spec.transport) });
        self
    }

    pub fn with_tool_filter(mut self, filter: ToolFilter) -> Self {
        self.tool_filter = filter;
        self
    }

    pub fn with_elicitation(mut self, sink: ElicitationSink) -> Self {
        self.elicitation = Some(sink);
        self
    }

    pub fn register_in_memory_server(
        mut self,
        name: impl Into<String>,
        factory: impl Fn(Vec<String>, &RuntimeServices) -> Result<McpServer, ServerFactoryError> + Send + Sync + 'static,
    ) -> Self {
        self.factories.insert(name.into(), Box::new(factory));
        self
    }

    pub fn with_agent_deps(mut self, deps: AgentDeps) -> Self {
        self.agent_deps = deps;
        self
    }

    pub fn spawn(self) -> Result<McpCatalog, McpSpawnError> {
        let exposures: HashMap<String, ToolExposure> =
            self.servers.iter().map(|server| (server.name.clone(), server.exposure.clone())).collect();
        let defers_tools = exposures.values().any(ToolExposure::defers_tools);
        let socket = defers_tools.then(UnixSocket::bind).transpose()?;
        let options = self.client_options();
        let services = RuntimeServices {
            root_dir: self.root_dir,
            agent_deps: self.agent_deps,
            deferred_tools_socket: socket.as_ref().map(|socket| socket.path().to_path_buf()),
        };
        let servers = self
            .servers
            .into_iter()
            .map(|server| server.build(&self.factories, &services))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(McpCatalog::new(McpGateway::new(servers, &options), self.tool_filter, exposures, socket))
    }

    fn insert(&mut self, server: ConfiguredServer) {
        match self.servers.iter_mut().find(|existing| existing.name == server.name) {
            Some(existing) => *existing = server,
            None => self.servers.push(server),
        }
    }

    fn client_options(&self) -> ClientOptions {
        let mut options =
            ClientOptions::default().implementation(Implementation::new("aether", env!("CARGO_PKG_VERSION")));
        if let Some(store) = self.agent_deps.oauth_credential_store.clone() {
            options = options.oauth_store(store);
        }
        if let Some(sink) = self.elicitation.clone() {
            options = options.elicitation(sink);
        }
        options
    }
}

type ServerFactory = Box<dyn Fn(Vec<String>, &RuntimeServices) -> Result<McpServer, ServerFactoryError> + Send + Sync>;

struct ConfiguredServer {
    name: String,
    exposure: ToolExposure,
    source: ServerSource,
}

impl ConfiguredServer {
    fn from_config(name: String, config: McpServerConfig, vars: &Vars, root_dir: &Path) -> Result<Self, ParseError> {
        let exposure = config.defer_tools().clone();
        Ok(Self { name, exposure, source: config.resolve(vars, root_dir)? })
    }

    fn build(
        self,
        factories: &HashMap<String, ServerFactory>,
        services: &RuntimeServices,
    ) -> Result<ServerSpec, McpSpawnError> {
        let transport = match self.source {
            ServerSource::Ready(transport) => transport,
            ServerSource::InMemory(args) => {
                let factory = factories
                    .get(&self.name)
                    .ok_or_else(|| McpSpawnError::InMemoryFactoryNotFound(self.name.clone()))?;
                let server = factory(args, services)
                    .map_err(|source| McpSpawnError::ServerFactory { name: self.name.clone(), source })?;
                Transport::InProcess(server)
            }
        };
        Ok(ServerSpec { name: self.name, transport })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aether_auth::{FakeOAuthCredentialStore, OAuthCredentialStorage};
    use mcp_utils::testing::FakeMcpServer;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use utils::mcp_status::McpServerStatus;

    fn config(json: &str) -> McpConfig {
        McpConfig::from_json(json).unwrap()
    }

    #[tokio::test]
    async fn in_memory_factory_runs_once_at_spawn_with_runtime_services() {
        let calls = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(None::<RuntimeServices>));
        let factory_calls = Arc::clone(&calls);
        let factory_received = Arc::clone(&received);
        let oauth_store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        let deps = AgentDeps::new(Arc::clone(&oauth_store), None);

        let builder = McpBuilder::new("/workspace")
            .with_agent_deps(deps)
            .register_in_memory_server("test", move |args, services| {
                factory_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(args, ["--root", "/workspace/tools"]);
                *factory_received.lock().unwrap() = Some(services.clone());
                Ok(FakeMcpServer::new().into())
            })
            .with_config(config(r#"{"servers":{"test":{"type":"in-memory","args":["--root","${WORKSPACE}/tools"]}}}"#))
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
        let mcp = McpBuilder::new("/workspace")
            .register_in_memory_server("test", move |_, services| {
                *factory_received.lock().unwrap() = Some(services.clone());
                Ok(FakeMcpServer::new().into())
            })
            .with_config(config(r#"{"servers":{"test":{"type":"in-memory","deferTools":true}}}"#))
            .unwrap()
            .spawn()
            .unwrap();

        let services = received.lock().unwrap().clone().expect("factory received runtime services");
        let socket = services.deferred_tools_socket.expect("factory receives the deferred tools socket");
        assert_eq!(socket, mcp.deferred_tools_socket().expect("the session serves deferred tools"));
        assert!(socket.exists());
    }

    #[tokio::test]
    async fn shutting_down_a_session_stops_the_deferred_socket_and_the_servers_for_every_clone() {
        let lazy = ServerSpec::in_process("lazy", FakeMcpServer::new());
        let mcp = McpBuilder::new("/workspace").with_server(lazy, ToolExposure::deferred_all()).spawn().unwrap();
        mcp.ready().await;
        let held = mcp.clone();
        let socket = mcp.deferred_tools_socket().expect("the session serves deferred tools").to_path_buf();

        mcp.shutdown().await;

        assert!(!socket.exists());
        assert_eq!(held.statuses().len(), 0);
    }

    #[tokio::test]
    async fn spawned_in_memory_servers_connect_in_the_background() {
        let mcp = McpBuilder::new("/workspace")
            .register_in_memory_server("test", |_, _| Ok(FakeMcpServer::math().into()))
            .with_config(config(r#"{"servers":{"test":{"type":"in-memory"}}}"#))
            .unwrap()
            .spawn()
            .unwrap();

        mcp.ready().await;

        assert_eq!(mcp.tool_definitions()[0].name, "test__add_numbers");
    }

    #[tokio::test]
    async fn ready_servers_and_config_servers_are_added_in_order() {
        let mcp = McpBuilder::new("/workspace")
            .register_in_memory_server("configured", |_, _| Ok(FakeMcpServer::new().into()))
            .with_servers(vec![ServerSpec::in_process("ready", FakeMcpServer::new())])
            .with_config(config(r#"{"servers":{"configured":{"type":"in-memory"}}}"#))
            .unwrap()
            .spawn()
            .unwrap();

        mcp.ready().await;

        assert_eq!(names(&mcp), ["ready", "configured"]);
    }

    #[tokio::test]
    async fn a_server_replaces_an_earlier_one_with_the_same_name_in_its_position() {
        let replaced = FakeMcpServer::new();
        let alive = replaced.alive();
        let replacement = FakeMcpServer::new().with_tool(mcp_utils::testing::FakeTool::new("replacement"));
        let mcp = McpBuilder::new("/workspace")
            .with_servers(vec![
                ServerSpec::in_process("math", replaced),
                ServerSpec::in_process("docs", FakeMcpServer::new()),
            ])
            .with_server(ServerSpec::in_process("math", replacement), ToolExposure::deferred_all())
            .spawn()
            .unwrap();

        mcp.ready().await;

        assert!(alive.upgrade().is_none());
        assert_eq!(names(&mcp), ["math", "docs"]);
        assert!(mcp.statuses()[0].deferred_tools);
        assert_eq!(mcp.statuses()[0].status, McpServerStatus::Connected { tool_count: 1 });
    }

    #[tokio::test]
    async fn missing_in_memory_factory_fails_at_spawn_with_server_name() {
        let builder = McpBuilder::new("/workspace")
            .with_config(config(r#"{"servers":{"custom":{"type":"in-memory"}}}"#))
            .unwrap();

        let Err(error) = builder.spawn() else {
            panic!("spawn should reject an unregistered factory");
        };
        assert!(matches!(error, McpSpawnError::InMemoryFactoryNotFound(ref server) if server == "custom"));
    }

    #[tokio::test]
    async fn rejected_in_memory_args_fail_spawn_with_server_name() {
        let builder = McpBuilder::new("/workspace")
            .register_in_memory_server("custom", |args, _| Err(format!("unexpected argument '{}'", args[0]).into()))
            .with_config(config(r#"{"servers":{"custom":{"type":"in-memory","args":["--bad"]}}}"#))
            .unwrap();

        let Err(error) = builder.spawn() else {
            panic!("spawn should surface the factory's error");
        };
        assert!(matches!(error, McpSpawnError::ServerFactory { ref name, .. } if name == "custom"));
        assert!(error.to_string().contains("unexpected argument '--bad'"), "unexpected error: {error}");
    }

    #[tokio::test]
    async fn config_args_expand_the_aether_home_var() {
        let home = aether_home().expect("Aether home resolves");

        let args = spawned_args(
            "/work",
            r#"{"servers":{"notes":{"type":"in-memory","args":["--dir","${AETHER_HOME}/skills"]}}}"#,
        );

        assert_eq!(args, ["--dir".to_string(), home.join("skills").to_string_lossy().into_owned()]);
    }

    fn names(mcp: &McpCatalog) -> Vec<String> {
        mcp.statuses().into_iter().map(|status| status.name).collect()
    }

    fn spawned_args(root_dir: &str, json: &str) -> Vec<String> {
        let received = Arc::new(Mutex::new(Vec::new()));
        let factory_received = Arc::clone(&received);
        McpBuilder::new(root_dir)
            .register_in_memory_server("notes", move |args, _| {
                *factory_received.lock().unwrap() = args;
                Ok(FakeMcpServer::new().into())
            })
            .with_config(config(json))
            .unwrap()
            .spawn()
            .unwrap();
        received.lock().unwrap().clone()
    }
}
