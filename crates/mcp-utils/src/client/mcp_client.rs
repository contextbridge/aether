// Don't use custom Result type here as we need to return rmcp::ErrorData
use rmcp::{
    ClientHandler, RoleClient,
    handler::client::progress::ProgressDispatcher,
    model::{ClientInfo, ElicitRequestParams, ElicitResult, ElicitationAction, ErrorData, ProgressNotificationParam},
    service::{NotificationContext, RequestContext},
};
use std::result::Result;
use tokio::sync::{mpsc, oneshot};

use crate::client::{ElicitationRequest, McpClientEvent};

pub struct McpClient {
    client_info: ClientInfo,
    server_name: String,
    pub(crate) progress_dispatcher: ProgressDispatcher,
    event_sender: mpsc::Sender<McpClientEvent>,
}

impl McpClient {
    pub fn new(client_info: ClientInfo, server_name: String, event_sender: mpsc::Sender<McpClientEvent>) -> Self {
        Self { client_info, server_name, progress_dispatcher: ProgressDispatcher::new(), event_sender }
    }

    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Dispatch an elicitation request through the shared event channel.
    ///
    /// Used by both the `create_elicitation` handler and the MRTR round loop
    /// in `call_tool_mrtr` to ensure the same user-facing flow.
    pub async fn dispatch_elicitation(&self, request: ElicitRequestParams) -> ElicitResult {
        let (response_tx, response_rx) = oneshot::channel();
        let elicitation_request =
            ElicitationRequest { server_name: self.server_name.clone(), request, response_sender: response_tx };

        if self.event_sender.send(McpClientEvent::Elicitation(elicitation_request)).await.is_err() {
            return cancel_result();
        }
        response_rx.await.unwrap_or_else(|_| cancel_result())
    }
}

pub fn cancel_result() -> ElicitResult {
    ElicitResult::new(ElicitationAction::Cancel)
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
        request: ElicitRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        Ok(self.dispatch_elicitation(request).await)
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
        let mut capabilities = ClientCapabilities::builder().enable_elicitation().build();
        if let Some(elicitation) = capabilities.elicitation.as_mut() {
            elicitation.form = Some(FormElicitationCapability::default());
            elicitation.url = Some(UrlElicitationCapability::default());
        }
        ClientInfo::new(capabilities, Implementation::new("test", "0.1.0"))
    }

    fn make_client(event_sender: mpsc::Sender<McpClientEvent>) -> McpClient {
        McpClient::new(test_client_info(), "test-server".to_string(), event_sender)
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

        let request = ElicitRequestParams::FormElicitationParams {
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

        let request = ElicitRequestParams::FormElicitationParams {
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

        let request = ElicitRequestParams::UrlElicitationParams {
            meta: None,
            message: "Auth".to_string(),
            url: "https://example.com/auth".to_string(),
            elicitation_id: "el-123".to_string(),
        };

        let handle = tokio::spawn(async move {
            let event = event_rx.recv().await.unwrap();
            let elicitation = unwrap_elicitation(event);
            assert_eq!(elicitation.server_name, "test-server");
            let _ = elicitation.response_sender.send(ElicitResult::new(ElicitationAction::Accept));
        });

        let result = client.dispatch_elicitation(request).await;
        handle.await.unwrap();
        assert_eq!(result.action, ElicitationAction::Accept);
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
