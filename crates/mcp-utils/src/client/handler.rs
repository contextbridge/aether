use super::ClientOptions;
use super::elicitation::{Elicitation, ElicitationRequest, cancelled};
use rmcp::{
    ClientHandler, RoleClient,
    handler::client::progress::ProgressDispatcher,
    model::{
        ClientConfig, CustomNotification, ElicitRequestParams, ElicitResult, ElicitationAction, ErrorData,
        InputRequest, InputRequests, InputResponses, ProgressNotificationParam, RequestMetaObject,
    },
    service::{MaybeSendFuture, NotificationContext, RequestContext},
};
use std::future::{Future, ready};
use tokio::sync::{mpsc, watch};

pub(crate) struct Handler {
    pub(crate) progress: ProgressDispatcher,
    info: ClientConfig,
    server: String,
    tx: Option<mpsc::Sender<Elicitation>>,
    tool_list_changed: watch::Sender<()>,
}

pub(crate) struct UnsupportedInput;

impl Handler {
    pub(crate) fn new(server: String, options: &ClientOptions) -> Self {
        Self {
            info: options.client_config(),
            server,
            progress: ProgressDispatcher::new(),
            tx: options.elicitations.clone(),
            tool_list_changed: watch::Sender::new(()),
        }
    }

    pub(crate) fn tool_list_changes(&self) -> watch::Receiver<()> {
        self.tool_list_changed.subscribe()
    }

    pub(crate) fn server(&self) -> &str {
        &self.server
    }

    pub(crate) async fn elicit_inputs(
        &self,
        requests: InputRequests,
    ) -> Result<(InputResponses, bool), UnsupportedInput> {
        let mut responses = InputResponses::new();
        let mut cancelled = false;
        for (key, request) in requests {
            let InputRequest::Elicitation(elicitation_request) = request else {
                return Err(UnsupportedInput);
            };
            let extension_meta = elicitation_request.extensions.get::<RequestMetaObject>().cloned();
            let result = self.elicit(elicitation_request.params, extension_meta).await;
            cancelled |= result.action == ElicitationAction::Cancel;
            let response = serde_json::to_value(&result).expect("ElicitResult serializes to JSON");
            responses.insert(key, response);
        }
        Ok((responses, cancelled))
    }

    /// Sends `request` to the host and waits for its answer, resolving to `Cancel` when there is
    /// no sink, the sink is closed, or the host drops the request.
    async fn elicit(&self, request: ElicitRequestParams, meta: Option<RequestMetaObject>) -> ElicitResult {
        let Some(events) = &self.tx else {
            return cancelled();
        };
        let (elicitation, response) = ElicitationRequest::new(self.server.clone(), with_meta(request, meta));
        if events.send(Elicitation::Request(Box::new(elicitation))).await.is_err() {
            return cancelled();
        }
        response.await.unwrap_or_else(|_| cancelled())
    }
}

impl ClientHandler for Handler {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }

    async fn on_progress(&self, params: ProgressNotificationParam, _context: NotificationContext<RoleClient>) -> () {
        self.progress.handle_notification(params).await;
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let meta = (!context.meta.is_empty()).then_some(context.meta);
        Ok(self.elicit(request, meta).await)
    }

    async fn on_custom_notification(
        &self,
        notification: CustomNotification,
        _context: NotificationContext<RoleClient>,
    ) {
        if notification.method != "notifications/elicitation/complete" {
            return;
        }
        let params: Option<ElicitationCompleteParams> =
            notification.params.and_then(|params| serde_json::from_value(params).ok());

        let Some(params) = params else {
            tracing::warn!("Ignoring malformed MCP elicitation completion notification");
            return;
        };

        if let Some(events) = &self.tx {
            let completion = Elicitation::Complete { server: self.server.clone(), id: params.elicitation_id };
            let _ = events.send(completion).await;
        }
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + MaybeSendFuture + '_ {
        self.tool_list_changed.send_replace(());
        ready(())
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ElicitationCompleteParams {
    elicitation_id: String,
}

fn with_meta(mut request: ElicitRequestParams, meta: Option<RequestMetaObject>) -> ElicitRequestParams {
    if let Some(meta) = meta {
        match &mut request {
            ElicitRequestParams::FormElicitationParams { meta: request_meta, .. }
            | ElicitRequestParams::UrlElicitationParams { meta: request_meta, .. } => {
                *request_meta = Some(meta);
            }
            _ => {}
        }
    }
    request
}
