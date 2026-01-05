// Don't use custom Result type here as we need to return rmcp::ErrorData
use rmcp::{
    ClientHandler, RoleClient,
    handler::client::progress::ProgressDispatcher,
    model::{
        ClientInfo, CreateElicitationRequestParam, CreateElicitationResult, ElicitationAction,
        ErrorData, ListRootsResult, ProgressNotificationParam,
    },
    service::{NotificationContext, RequestContext},
};
use std::result::Result;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot, RwLock};

use crate::mcp::ElicitationRequest;
use rmcp::model::Root;

pub struct McpClient {
    client_info: ClientInfo,
    pub progress_dispatcher: ProgressDispatcher,
    elicitation_sender: mpsc::Sender<ElicitationRequest>,
    /// Roots advertised to MCP servers
    roots: Arc<RwLock<Vec<Root>>>,
}

impl McpClient {
    pub fn new(
        client_info: ClientInfo,
        elicitation_sender: mpsc::Sender<ElicitationRequest>,
        roots: Arc<RwLock<Vec<Root>>>,
    ) -> Self {
        Self {
            client_info,
            progress_dispatcher: ProgressDispatcher::new(),
            elicitation_sender,
            roots,
        }
    }
}

impl ClientHandler for McpClient {
    fn get_info(&self) -> ClientInfo {
        self.client_info.clone()
    }

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> () {
        self.progress_dispatcher.handle_notification(params).await;
    }

    async fn create_elicitation(
        &self,
        request: CreateElicitationRequestParam,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, ErrorData> {
        let (response_tx, response_rx) = oneshot::channel();
        let elicitation_request = ElicitationRequest {
            request,
            response_sender: response_tx,
        };

        match self.elicitation_sender.send(elicitation_request).await {
            Ok(_) => match response_rx.await {
                Ok(result) => Ok(result),
                Err(_) => Ok(CreateElicitationResult {
                    action: ElicitationAction::Decline,
                    content: None,
                }),
            },

            Err(_) => Ok(CreateElicitationResult {
                action: ElicitationAction::Decline,
                content: None,
            }),
        }
    }

    async fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> Result<ListRootsResult, ErrorData> {
        let roots = self.roots.read().await;

        Ok(ListRootsResult {
            roots: roots.clone(),
        })
    }
}
