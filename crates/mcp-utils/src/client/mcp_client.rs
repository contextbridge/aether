// Don't use custom Result type here as we need to return rmcp::ErrorData
use rmcp::{
    ClientHandler, RoleClient,
    handler::client::progress::ProgressDispatcher,
    model::{
        ClientInfo, ElicitRequestParams, ElicitResult, ElicitationAction, ErrorData, InputRequest, InputRequests,
        InputResponses, ProgressNotificationParam,
    },
    service::{NotificationContext, RequestContext},
};
use std::result::Result;
use tokio::sync::{mpsc, oneshot};

use crate::client::error::McpError;
use crate::client::{ElicitationRequest, McpClientEvent};

pub struct McpClient {
    client_info: ClientInfo,
    server_name: String,
    pub progress_dispatcher: ProgressDispatcher,
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
    pub async fn dispatch_elicitation(&self, request: ElicitRequestParams) -> ElicitResult {
        let (response_tx, response_rx) = oneshot::channel();
        let elicitation_request =
            ElicitationRequest { server_name: self.server_name.clone(), request, response_sender: response_tx };

        if self.event_sender.send(McpClientEvent::Elicitation(elicitation_request)).await.is_err() {
            return cancel_result();
        }
        response_rx.await.unwrap_or_else(|_| cancel_result())
    }

    /// Fulfill all embedded MRTR input requests through the existing host UI.
    /// Every request is validated before the first form is displayed.
    pub async fn fulfill_mrtr_input_requests(&self, requests: InputRequests) -> Result<InputResponses, McpError> {
        for (key, request) in &requests {
            if !matches!(request, InputRequest::Elicitation(_)) {
                return Err(McpError::UnsupportedMrtrInput(format!("key {key} uses an unsupported request method")));
            }
        }

        let mut responses = InputResponses::new();
        for (key, request) in requests {
            let InputRequest::Elicitation(elicitation) = request else {
                unreachable!("MRTR requests were preflighted above")
            };
            let is_form = matches!(&elicitation.params, ElicitRequestParams::FormElicitationParams { .. });
            let result = self.dispatch_elicitation(elicitation.params).await;
            if is_form
                && result.action == ElicitationAction::Accept
                && !matches!(result.content, Some(serde_json::Value::Object(_)))
            {
                return Err(McpError::MalformedMrtrInput(
                    "accepted form response must provide object-shaped content".into(),
                ));
            }
            let value =
                serde_json::to_value(result).map_err(|error| McpError::MalformedMrtrInput(error.to_string()))?;
            responses.insert(key, value);
        }
        Ok(responses)
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

        let request = ElicitRequestParams::FormElicitationParams {
            meta: None,
            message: "test".to_string(),
            requested_schema: ElicitationSchema::new(BTreeMap::new()),
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
