use super::state::AcpState;
use acp_utils::notifications::{
    McpRequest, PromptSearchParams, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams,
};
use agent_client_protocol::schema::v2::{
    CancelSessionNotification, CloseSessionRequest, InitializeRequest, ListSessionsRequest, LoginAuthRequest,
    LogoutAuthRequest, NewSessionRequest, PromptRequest, ResumeSessionRequest, SetSessionConfigOptionRequest,
};
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{
    self as acp, Agent, Builder, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled, JsonRpcResponse, NullRun,
    Responder,
};
use std::future::Future;
use std::sync::Arc;

pub(crate) fn acp_agent_builder(state: Arc<AcpState>) -> Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
    Agent.v2().name("aether-acp").with_handler(AcpHandlers(state))
}

struct AcpHandlers(Arc<AcpState>);

impl HandleDispatchFrom<Client> for AcpHandlers {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<Client>,
    ) -> Result<Handled<Dispatch>, acp::Error> {
        let state = &self.0;
        MatchDispatchFrom::new(message, &cx)
            .if_request(async |req: InitializeRequest, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.initialize(req).await })
            })
            .await
            .if_request(async |req: LoginAuthRequest, responder| {
                let state = state.clone();
                let connection = cx.clone();
                spawn_response(&cx, responder, async move { state.login(req, &connection).await })
            })
            .await
            .if_request(async |req: NewSessionRequest, responder| {
                let state = state.clone();
                let connection = cx.clone();
                spawn_response(&cx, responder, async move { state.new_session(req, &connection).await })
            })
            .await
            .if_request(async |req: ListSessionsRequest, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.list_sessions(&req) })
            })
            .await
            .if_request(async |req: LogoutAuthRequest, responder| {
                let state = state.clone();
                let connection = cx.clone();
                spawn_response(&cx, responder, async move { state.logout(req, &connection).await })
            })
            .await
            .if_request(async |req: ResumeSessionRequest, responder| {
                let state = state.clone();
                let connection = cx.clone();
                spawn_response(&cx, responder, async move { state.resume_session(req, &connection).await })
            })
            .await
            .if_request(async |req: CloseSessionRequest, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.close_session(req).await })
            })
            .await
            .if_request(async |req: PromptRequest, responder| {
                state.route_prompt(req, responder).await;
                Ok(())
            })
            .await
            .if_request(async |req: SetSessionConfigOptionRequest, responder| {
                let state = state.clone();
                cx.spawn(async move {
                    state.set_session_config_option(req, responder).await;
                    Ok(())
                })
            })
            .await
            .if_request(async |req: PromptSearchParams, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.search_prompts(&req) })
            })
            .await
            .if_request(async |req: SessionPreviewParams, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.session_preview(&req) })
            })
            .await
            .if_request(async |req: WorkspaceListParams, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.workspace_list(&req).await })
            })
            .await
            .if_request(async |req: WorkspaceMoveParams, responder| {
                let state = state.clone();
                spawn_response(&cx, responder, async move { state.workspace_move(&req).await })
            })
            .await
            .if_notification(async |notification: CancelSessionNotification| {
                let _ = state.cancel(notification).await;
                Ok(())
            })
            .await
            .if_notification(async |notification: McpRequest| {
                let _ = state.on_mcp_request(notification).await;
                Ok(())
            })
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "AcpHandlers"
    }
}

fn spawn_response<T, U>(cx: &ConnectionTo<Client>, responder: Responder<T>, future: U) -> Result<(), acp::Error>
where
    T: JsonRpcResponse + Send + 'static,
    U: Future<Output = Result<T, acp::Error>> + Send + 'static,
{
    cx.spawn(async move {
        if let Err(e) = responder.respond_with_result(future.await) {
            tracing::warn!("failed to send ACP response: {e:?}");
        }
        Ok(())
    })
}
