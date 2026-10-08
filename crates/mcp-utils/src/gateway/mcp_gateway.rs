use super::catalog::McpCatalog;
use super::deferred_tools::DeferredToolsMcp;
use super::servers::Servers;
use super::{ToolExposure, ToolFilter, namespaced, split_namespaced};
use crate::McpError;
use crate::client::{ClientOptions, ToolCall, ToolCallOptions, Transport};
use crate::server::McpServer;
use futures::future::try_join_all;
use rmcp::model::{GetPromptResult, Prompt};
use serde_json::{Map, Value};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;
use tokio::sync::watch;

#[derive(Clone)]
pub struct McpGateway {
    servers: Arc<Mutex<Servers>>,
    catalog: watch::Receiver<Arc<McpCatalog>>,
}

#[derive(Debug, Clone)]
pub struct ServerSpec {
    pub name: String,
    pub transport: Transport,
    pub exposure: ToolExposure,
}

impl McpGateway {
    pub fn new(client_options: ClientOptions, tool_filter: ToolFilter) -> Self {
        let (servers, catalog) = Servers::new(client_options, tool_filter);
        Self { servers: Arc::new(Mutex::new(servers)), catalog }
    }

    pub fn add_servers(&self, servers: Vec<ServerSpec>) -> Result<(), McpError> {
        self.lock().add(servers, &Arc::downgrade(&self.servers))
    }

    pub async fn ready(&self) -> Arc<McpCatalog> {
        let mut catalog = self.catalog.clone();
        let settled = catalog.wait_for(|catalog| catalog.is_settled()).await.map(|settled| Arc::clone(&settled));
        settled.unwrap_or_else(|_| Arc::clone(&catalog.borrow()))
    }

    pub async fn authenticate(&self, server: &str) -> Result<(), McpError> {
        let outcome = self.lock().authenticate(server, &Arc::downgrade(&self.servers))?;
        outcome.await.map_err(|_| McpError::GatewayClosed)?
    }

    pub async fn shutdown(&self) {
        let closing = self.lock().close();
        closing.await;
    }

    pub fn catalog(&self) -> Arc<McpCatalog> {
        Arc::clone(&self.catalog.borrow())
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<McpCatalog>> {
        let mut catalog = self.catalog.clone();
        catalog.mark_unchanged();
        catalog
    }

    pub fn call_tool(
        &self,
        name: &str,
        args: Map<String, Value>,
        options: ToolCallOptions,
    ) -> Result<ToolCall, McpError> {
        self.lock().call_tool(name, args, options)
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, McpError> {
        let clients = self.lock().clients()?;
        let listed = try_join_all(clients.iter().map(|client| async move {
            let prompts = client.list_prompts().await?;
            Ok::<_, McpError>(prompts.into_iter().map(move |mut prompt| {
                prompt.name = namespaced(client.name(), &prompt.name);
                prompt
            }))
        }))
        .await?;
        Ok(listed.into_iter().flatten().collect())
    }

    pub async fn get_prompt(&self, name: &str, args: Map<String, Value>) -> Result<GetPromptResult, McpError> {
        let (server, prompt) = split_namespaced(name).ok_or_else(|| McpError::NotNamespaced(name.to_string()))?;
        let client = self.lock().client(server)?;
        client.get_prompt(prompt, args).await
    }

    pub fn deferred_tools_server(&self, call_timeout: Option<Duration>) -> McpServer {
        McpServer::new(DeferredToolsMcp::new(Arc::downgrade(&self.servers), self.catalog.clone(), call_timeout))
    }

    fn lock(&self) -> MutexGuard<'_, Servers> {
        self.servers.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
