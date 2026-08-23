use super::run_mcp_task::ManagerCommand;
use futures::{Stream, future::join_all};
use mcp_utils::client::{CallToolError, CallToolOptions, McpError, McpSnapshot, ToolCallEvent, ToolRoute, call_tool};
use rmcp::model::{GetPromptRequestParams, GetPromptResult, Prompt};
use serde_json::{Map, Value};
use std::{pin::Pin, sync::Arc};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, watch};

pub type ToolCallStream = Pin<Box<dyn Stream<Item = ToolCallEvent> + Send>>;

#[derive(Clone)]
pub struct McpHandle {
    control_tx: mpsc::Sender<ManagerCommand>,
    snapshot_rx: watch::Receiver<Arc<McpSnapshot>>,
}

#[derive(Debug, Error)]
pub enum McpHandleError {
    #[error(transparent)]
    Route(#[from] McpError),
    #[error("MCP manager is unavailable")]
    ManagerUnavailable,
    #[error("failed to list prompts for {server}: {message}")]
    PromptList { server: String, message: String },
    #[error("failed to get prompt '{prompt}' from {server}: {message}")]
    PromptGet { server: String, prompt: String, message: String },
}

impl McpHandle {
    pub(super) fn new(
        control_tx: mpsc::Sender<ManagerCommand>,
        snapshot_rx: watch::Receiver<Arc<McpSnapshot>>,
    ) -> Self {
        Self { control_tx, snapshot_rx }
    }

    pub fn snapshot(&self) -> Arc<McpSnapshot> {
        self.snapshot_rx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<McpSnapshot>> {
        self.snapshot_rx.clone()
    }

    pub fn call(&self, route: ToolRoute, arguments: Map<String, Value>, options: CallToolOptions) -> ToolCallStream {
        match self.snapshot().resolve(route, arguments) {
            Ok((client, params)) => Box::pin(call_tool(client, params, options)),
            Err(error) => Box::pin(futures::stream::once(async move {
                ToolCallEvent::Complete(Err(CallToolError::Unavailable {
                    message: format!("Failed to resolve tool: {error}"),
                }))
            })),
        }
    }

    pub fn call_model_visible(
        &self,
        namespaced_name: String,
        arguments_json: &str,
        options: CallToolOptions,
    ) -> ToolCallStream {
        let parsed = serde_json::from_str::<Value>(arguments_json)
            .map_err(McpError::from)
            .and_then(|value| {
                value
                    .as_object()
                    .cloned()
                    .ok_or_else(|| McpError::JsonError("tool arguments must be a JSON object".to_string()))
            })
            .map(|arguments| (ToolRoute::ModelVisible { namespaced_name }, arguments));
        match parsed {
            Ok((route, arguments)) => self.call(route, arguments, options),
            Err(error) => failed_call(error),
        }
    }

    pub async fn list_prompts(&self) -> Result<Vec<Prompt>, McpHandleError> {
        let futures = self.snapshot().clients_with_prompts().into_iter().map(|(server, client)| async move {
            let prompts = client
                .list_all_prompts()
                .await
                .map_err(|error| McpHandleError::PromptList { server: server.clone(), message: error.to_string() })?;
            Ok::<_, McpHandleError>(
                prompts
                    .into_iter()
                    .map(|prompt| {
                        Prompt::new(format!("{server}__{}", prompt.name), prompt.description, prompt.arguments)
                    })
                    .collect::<Vec<_>>(),
            )
        });
        let mut prompts = Vec::new();
        for result in join_all(futures).await {
            prompts.extend(result?);
        }
        Ok(prompts)
    }

    pub async fn get_prompt(
        &self,
        name: &str,
        arguments: Option<Map<String, Value>>,
    ) -> Result<GetPromptResult, McpHandleError> {
        let (prompt, client) = self.snapshot().client_for_prompt(name)?;
        let server = client.service().server_name().to_string();
        let mut request = GetPromptRequestParams::new(prompt.clone());
        if let Some(arguments) = arguments {
            request = request.with_arguments(arguments);
        }
        client.get_prompt(request).await.map_err(|error| McpHandleError::PromptGet {
            server,
            prompt,
            message: error.to_string(),
        })
    }

    pub async fn authenticate_server(&self, name: &str) -> Result<(), McpHandleError> {
        let (tx, rx) = oneshot::channel();
        self.control_tx
            .send(ManagerCommand::AuthenticateServer { name: name.to_string(), tx })
            .await
            .map_err(|_| McpHandleError::ManagerUnavailable)?;
        rx.await.map_err(|_| McpHandleError::ManagerUnavailable)?.map_err(McpHandleError::Route)
    }
}

fn failed_call(error: McpError) -> ToolCallStream {
    Box::pin(futures::stream::once(async move {
        ToolCallEvent::Complete(Err(CallToolError::Unavailable { message: format!("Failed to resolve tool: {error}") }))
    }))
}
