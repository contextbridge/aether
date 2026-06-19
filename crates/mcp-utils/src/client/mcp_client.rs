// Don't use custom Result type here as we need to return rmcp::ErrorData
use rmcp::{
    ClientHandler, RoleClient,
    handler::client::progress::ProgressDispatcher,
    model::{
        ClientInfo, CreateElicitationRequestParams, CreateElicitationResult, ElicitationAction,
        ElicitationResponseNotificationParam, ErrorData, ListRootsResult, ProgressNotificationParam,
    },
    service::{NotificationContext, RequestContext},
};
use std::result::Result;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, oneshot};

use crate::client::{ElicitationRequest, McpClientEvent};
use rmcp::model::Root;

pub struct McpClient {
    client_info: ClientInfo,
    server_name: String,
    pub progress_dispatcher: ProgressDispatcher,
    event_sender: mpsc::Sender<McpClientEvent>,
    /// Roots advertised to MCP servers
    roots: Arc<RwLock<Vec<Root>>>,
}

impl McpClient {
    pub fn new(
        client_info: ClientInfo,
        server_name: String,
        event_sender: mpsc::Sender<McpClientEvent>,
        roots: Arc<RwLock<Vec<Root>>>,
    ) -> Self {
        Self { client_info, server_name, progress_dispatcher: ProgressDispatcher::new(), event_sender, roots }
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Dispatch an elicitation request through the shared event channel.
    ///
    /// Used by both the `create_elicitation` handler and the `-32042`
    /// `URL_ELICITATION_REQUIRED` error path to ensure the same user-facing flow.
    pub async fn dispatch_elicitation(&self, request: CreateElicitationRequestParams) -> CreateElicitationResult {
        let (response_tx, response_rx) = oneshot::channel();
        let elicitation_request =
            ElicitationRequest { server_name: self.server_name.clone(), request, response_sender: response_tx };

        if self.event_sender.send(McpClientEvent::Elicitation(elicitation_request)).await.is_err() {
            return cancel_result();
        }
        response_rx.await.unwrap_or_else(|_| cancel_result())
    }

    /// Forward a URL elicitation completion through the shared event channel.
    ///
    /// Split out from `on_url_elicitation_notification_complete` so it can be
    /// tested without constructing a `NotificationContext`.
    pub async fn forward_url_elicitation_complete(&self, elicitation_id: String) {
        let event = McpClientEvent::UrlElicitationComplete(super::UrlElicitationCompleteParams {
            server_name: self.server_name.clone(),
            elicitation_id,
        });
        if self.event_sender.send(event).await.is_err() {
            tracing::warn!("Failed to forward URL elicitation completion: receiver dropped");
        }
    }
}

pub fn cancel_result() -> CreateElicitationResult {
    CreateElicitationResult { action: ElicitationAction::Cancel, content: None, meta: Option::default() }
}

impl ClientHandler for McpClient {
    fn get_info(&self) -> ClientInfo {
        self.client_info.clone()
    }

    async fn on_progress(&self, params: ProgressNotificationParam, _context: NotificationContext<RoleClient>) -> () {
        self.progress_dispatcher.handle_notification(params).await;
    }

    async fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, ErrorData> {
        Ok(self.dispatch_elicitation(request).await)
    }

    async fn on_url_elicitation_notification_complete(
        &self,
        params: ElicitationResponseNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        self.forward_url_elicitation_complete(params.elicitation_id).await;
    }

    async fn list_roots(&self, _context: RequestContext<RoleClient>) -> Result<ListRootsResult, ErrorData> {
        let roots = self.roots.read().await;

        Ok(ListRootsResult::new(roots.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{
        ClientCapabilities, ElicitationSchema, FormElicitationCapability, Implementation, UrlElicitationCapability,
    };
    use std::collections::BTreeMap;

    fn test_client_info() -> ClientInfo {
        let mut capabilities = ClientCapabilities::builder().enable_elicitation().enable_roots().build();
        if let Some(elicitation) = capabilities.elicitation.as_mut() {
            elicitation.form = Some(FormElicitationCapability::default());
            elicitation.url = Some(UrlElicitationCapability::default());
        }
        ClientInfo::new(capabilities, Implementation::new("test", "0.1.0"))
    }

    fn make_client(event_sender: mpsc::Sender<McpClientEvent>) -> McpClient {
        McpClient::new(test_client_info(), "test-server".to_string(), event_sender, Arc::new(RwLock::new(Vec::new())))
    }

    fn unwrap_elicitation(event: McpClientEvent) -> ElicitationRequest {
        match event {
            McpClientEvent::Elicitation(req) => req,
            other => panic!("expected Elicitation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_elicitation_dropped_sender_returns_cancel() {
        let (event_tx, _) = mpsc::channel(1);
        let client = make_client(event_tx);

        let request = CreateElicitationRequestParams::FormElicitationParams {
            meta: None,
            message: "test".to_string(),
            requested_schema: ElicitationSchema::new(BTreeMap::new()),
        };

        let result = client.dispatch_elicitation(request).await;
        assert_eq!(result.action, ElicitationAction::Cancel, "dropped sender should return Cancel, not Decline");
        assert!(result.content.is_none());
    }

    #[tokio::test]
    async fn dispatch_elicitation_dropped_receiver_returns_cancel() {
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let client = make_client(event_tx);

        let request = CreateElicitationRequestParams::FormElicitationParams {
            meta: None,
            message: "test".to_string(),
            requested_schema: ElicitationSchema::new(BTreeMap::new()),
        };

        let handle = tokio::spawn(async move {
            let event = event_rx.recv().await.unwrap();
            let elicitation = unwrap_elicitation(event);
            drop(elicitation.response_sender);
        });

        let result = client.dispatch_elicitation(request).await;
        handle.await.unwrap();

        assert_eq!(result.action, ElicitationAction::Cancel, "dropped receiver should return Cancel, not Decline");
        assert!(result.content.is_none());
    }

    #[tokio::test]
    async fn dispatch_elicitation_forwards_request_with_server_name() {
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let client = make_client(event_tx);

        let request = CreateElicitationRequestParams::UrlElicitationParams {
            meta: None,
            message: "Auth".to_string(),
            url: "https://example.com/auth".to_string(),
            elicitation_id: "el-123".to_string(),
        };

        let handle = tokio::spawn(async move {
            let event = event_rx.recv().await.unwrap();
            let elicitation = unwrap_elicitation(event);
            assert_eq!(elicitation.server_name, "test-server");
            let _ = elicitation.response_sender.send(CreateElicitationResult {
                action: ElicitationAction::Accept,
                content: None,
                meta: Option::default(),
            });
        });

        let result = client.dispatch_elicitation(request).await;
        handle.await.unwrap();
        assert_eq!(result.action, ElicitationAction::Accept);
    }

    #[tokio::test]
    async fn forward_url_elicitation_complete_uses_server_name_and_id() {
        let (event_tx, mut event_rx) = mpsc::channel(1);
        let client = make_client(event_tx);

        client.forward_url_elicitation_complete("el-456".to_string()).await;

        let event = event_rx.recv().await.unwrap();
        match event {
            McpClientEvent::UrlElicitationComplete(params) => {
                assert_eq!(params.server_name, "test-server");
                assert_eq!(params.elicitation_id, "el-456");
            }
            other => panic!("expected UrlElicitationComplete, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn forward_url_elicitation_complete_swallows_dropped_receiver() {
        let (event_tx, event_rx) = mpsc::channel(1);
        drop(event_rx);
        let client = make_client(event_tx);

        // Should not panic even though the receiver is dropped.
        client.forward_url_elicitation_complete("el-gone".to_string()).await;
    }

    #[test]
    fn capabilities_include_form_and_url() {
        let info = test_client_info();
        let caps = &info.capabilities;
        let elicitation = caps.elicitation.as_ref().expect("elicitation capability should be set");
        assert!(elicitation.form.is_some(), "form capability should be advertised");
        assert!(elicitation.url.is_some(), "url capability should be advertised");
    }
}
