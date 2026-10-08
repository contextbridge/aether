use super::catalog::{CatalogEntry, CatalogTool, McpCatalog};
use super::connection::{self, Credentials};
use super::{ServerSpec, ToolFilter};
use crate::McpError;
use crate::client::{ClientOptions, McpClient, ToolCall, ToolCallOptions};
use aether_auth::OAuthError;
use futures::future::{BoxFuture, FutureExt, join_all};
use rmcp::model::Tool;
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, watch};
use tokio_util::task::AbortOnDropHandle;
use utils::mcp_status::{McpServerAuthCapability, McpServerStatus, McpServerStatusEntry};

pub(super) struct Servers {
    list: Vec<Server>,
    client_options: ClientOptions,
    tool_filter: ToolFilter,
    catalog: Option<watch::Sender<Arc<McpCatalog>>>,
    next_generation: u64,
}

pub(super) type Shared = Weak<Mutex<Servers>>;

type Outcome = oneshot::Sender<Result<(), McpError>>;

impl Servers {
    pub(super) fn new(
        client_options: ClientOptions,
        tool_filter: ToolFilter,
    ) -> (Self, watch::Receiver<Arc<McpCatalog>>) {
        let mut servers = Self { list: Vec::new(), client_options, tool_filter, catalog: None, next_generation: 0 };
        let (catalog, catalog_rx) = watch::channel(Arc::new(servers.snapshot()));
        servers.catalog = Some(catalog);
        (servers, catalog_rx)
    }

    pub(super) fn add(&mut self, specs: Vec<ServerSpec>, shared: &Shared) -> Result<(), McpError> {
        self.ensure_open()?;
        for spec in specs {
            let server = self.connect(spec, Credentials::Existing, shared);
            match self.position(&server.spec.name) {
                Some(index) => std::mem::replace(&mut self.list[index], server).cancel_authentication(),
                None => self.list.push(server),
            }
        }
        self.publish();
        Ok(())
    }

    pub(super) fn authenticate(
        &mut self,
        name: &str,
        shared: &Shared,
    ) -> Result<oneshot::Receiver<Result<(), McpError>>, McpError> {
        self.ensure_open()?;
        let index = self.position(name).ok_or_else(|| McpError::ServerNotFound(name.to_string()))?;
        let existing = &self.list[index];
        if !existing.entry.status.can_authenticate() {
            return Err(McpError::OAuthUnavailable { server: name.to_string() });
        }
        let credentials = Credentials::SignIn { challenge: existing.challenge.clone() };
        let mut server = self.connect(existing.spec.clone(), credentials, shared);
        let (outcome, receiver) = oneshot::channel();
        server.outcome = Some(outcome);
        std::mem::replace(&mut self.list[index], server).cancel_authentication();
        self.publish();
        Ok(receiver)
    }

    pub(super) fn settle(&mut self, generation: u64, result: Result<(McpClient, Vec<Tool>), McpError>) {
        let Some(server) = current(&mut self.list, generation) else { return };
        let outcome = server.settle(result, &self.client_options, &self.tool_filter);
        let waiter = server.outcome.take();
        self.publish();
        if let Some(waiter) = waiter {
            let _ = waiter.send(outcome);
        }
    }

    pub(super) fn refresh(&mut self, generation: u64, result: Result<Vec<Tool>, McpError>) {
        let Some(server) = current(&mut self.list, generation) else { return };
        let Some(client) = server.client.clone() else { return };
        match result {
            Ok(tools) => server.connected(client, &tools, server.entry.status.auth_capability, &self.tool_filter),
            Err(error) => {
                tracing::warn!(server = server.spec.name, %error, "Failed to refresh MCP tools; retaining previous catalog");
                return;
            }
        }
        self.publish();
    }

    pub(super) fn close(&mut self) -> BoxFuture<'static, ()> {
        let clients = self.list.drain(..).filter_map(|server| server.client).collect::<Vec<_>>();
        self.publish();
        let catalog = self.catalog.take();
        async move {
            join_all(clients.iter().map(McpClient::close)).await;
            drop(catalog);
        }
        .boxed()
    }

    pub(super) fn call_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        options: ToolCallOptions,
    ) -> Result<ToolCall, McpError> {
        let (client, tool) = self.resolve(name)?;
        if tool.deferred {
            return Err(McpError::ToolNotFound(name.to_string()));
        }
        Ok(client.call_tool(&tool.local_name, args, options))
    }

    pub(super) fn call_deferred_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        options: ToolCallOptions,
    ) -> Result<ToolCall, McpError> {
        let (client, tool) = self.resolve(name)?;
        if !tool.deferred {
            return Err(McpError::ToolNotDeferred { tool: tool.local_name.clone(), direct_name: name.to_string() });
        }
        Ok(client.call_tool(&tool.local_name, args, options))
    }

    pub(super) fn clients(&self) -> Result<Vec<McpClient>, McpError> {
        self.ensure_open()?;
        Ok(self.list.iter().filter_map(|server| server.client.clone()).collect())
    }

    pub(super) fn client(&self, name: &str) -> Result<McpClient, McpError> {
        self.ensure_open()?;
        self.list
            .iter()
            .find(|server| server.spec.name == name)
            .and_then(|server| server.client.clone())
            .ok_or_else(|| McpError::ServerNotFound(name.to_string()))
    }

    fn ensure_open(&self) -> Result<(), McpError> {
        if self.catalog.is_none() {
            return Err(McpError::GatewayClosed);
        }
        Ok(())
    }

    fn resolve(&self, name: &str) -> Result<(&McpClient, &CatalogTool), McpError> {
        self.ensure_open()?;
        self.list
            .iter()
            .find_map(|server| Some((server.client.as_ref()?, server.entry.tool(name)?)))
            .ok_or_else(|| McpError::ToolNotFound(name.to_string()))
    }

    fn connect(&mut self, spec: ServerSpec, credentials: Credentials, shared: &Shared) -> Server {
        self.next_generation += 1;
        let (status, auth) = match credentials {
            Credentials::Existing => (McpServerStatus::Connecting, McpServerAuthCapability::Unavailable),
            Credentials::SignIn { .. } => (McpServerStatus::Authenticating, McpServerAuthCapability::OAuth),
        };
        let connection = AbortOnDropHandle::new(tokio::spawn(connection::run(
            Weak::clone(shared),
            self.next_generation,
            spec.name.clone(),
            spec.transport.clone(),
            credentials,
            self.client_options.clone(),
        )));
        let entry = Arc::new(CatalogEntry::disconnected(status_entry(&spec, status, auth)));
        Server {
            spec,
            generation: self.next_generation,
            challenge: None,
            client: None,
            entry,
            _connection: connection,
            outcome: None,
        }
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.list.iter().position(|server| server.spec.name == name)
    }

    fn publish(&self) {
        if let Some(catalog) = &self.catalog {
            catalog.send_replace(Arc::new(self.snapshot()));
        }
    }

    fn snapshot(&self) -> McpCatalog {
        let entries = self.list.iter().map(|server| Arc::clone(&server.entry)).collect();
        let settled = !self.list.iter().any(|server| server.entry.status.status == McpServerStatus::Connecting);
        McpCatalog::new(entries, settled)
    }
}

impl Drop for Servers {
    fn drop(&mut self) {
        let closing = self.close();
        if let Ok(runtime) = Handle::try_current() {
            runtime.spawn(closing);
        }
    }
}

/// Runs `f` against the servers unless every gateway handle has been dropped.
pub(super) fn with_servers<T>(servers: &Shared, f: impl FnOnce(&mut Servers) -> T) -> Option<T> {
    let servers = servers.upgrade()?;
    let result = f(&mut servers.lock().unwrap_or_else(PoisonError::into_inner));
    Some(result)
}

struct Server {
    spec: ServerSpec,
    generation: u64,
    challenge: Option<String>,
    client: Option<McpClient>,
    entry: Arc<CatalogEntry>,
    _connection: AbortOnDropHandle<()>,
    outcome: Option<Outcome>,
}

impl Server {
    fn settle(
        &mut self,
        result: Result<(McpClient, Vec<Tool>), McpError>,
        options: &ClientOptions,
        filter: &ToolFilter,
    ) -> Result<(), McpError> {
        let offers_oauth =
            matches!(result, Ok(_) | Err(McpError::AuthRequired { .. })) && self.spec.transport.supports_oauth(options);
        let auth = if offers_oauth || self.entry.status.can_authenticate() {
            McpServerAuthCapability::OAuth
        } else {
            McpServerAuthCapability::Unavailable
        };
        match result {
            Ok((client, tools)) => {
                self.connected(client, &tools, auth, filter);
                Ok(())
            }
            Err(McpError::AuthRequired { server, challenge }) => {
                tracing::warn!(server, "MCP server needs OAuth");
                self.challenge.clone_from(&challenge);
                self.disconnected(McpServerStatus::NeedsOAuth, auth);
                Err(McpError::AuthRequired { server, challenge })
            }
            Err(error) => {
                tracing::warn!(server = self.spec.name, %error, "MCP server failed to connect");
                self.disconnected(McpServerStatus::Failed { error: error.to_string() }, auth);
                Err(error)
            }
        }
    }

    fn connected(&mut self, client: McpClient, tools: &[Tool], auth: McpServerAuthCapability, filter: &ToolFilter) {
        let status = status_entry(&self.spec, McpServerStatus::Connected { tool_count: tools.len() }, auth);
        self.entry = Arc::new(CatalogEntry::connected(status, &client, tools, &self.spec.exposure, filter));
        self.client = Some(client);
    }

    fn disconnected(&mut self, status: McpServerStatus, auth: McpServerAuthCapability) {
        self.entry = Arc::new(CatalogEntry::disconnected(status_entry(&self.spec, status, auth)));
        self.client = None;
    }

    fn cancel_authentication(mut self) {
        if let Some(outcome) = self.outcome.take() {
            let server = self.spec.name;
            let _ = outcome.send(Err(McpError::Auth { server, source: OAuthError::UserCancelled }));
        }
    }
}

fn status_entry(spec: &ServerSpec, status: McpServerStatus, auth: McpServerAuthCapability) -> McpServerStatusEntry {
    McpServerStatusEntry::new(&spec.name, status)
        .with_auth_capability(auth)
        .with_deferred_tools(spec.exposure.has_deferred_tools())
}

fn current(servers: &mut [Server], generation: u64) -> Option<&mut Server> {
    servers.iter_mut().find(|server| server.generation == generation)
}
