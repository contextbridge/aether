use super::state::AcpState;
use acp_utils::notifications::{McpRequest, PromptSearchParams};
use agent_client_protocol::schema::{
    AuthenticateRequest, CancelNotification, InitializeRequest, ListSessionsRequest, LoadSessionRequest,
    NewSessionRequest, PromptRequest, SetSessionConfigOptionRequest,
};
use agent_client_protocol::{
    self as acp, Agent, Builder, Client, ConnectionTo, HandleDispatchFrom, JsonRpcResponse, NullRun, Responder,
};
use std::future::Future;
use std::sync::Arc;

#[allow(clippy::too_many_lines)]
pub(crate) fn acp_agent_builder(state: Arc<AcpState>) -> Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
    Agent
        .builder()
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: InitializeRequest, responder, cx| {
                    let state = state.clone();
                    spawn_response(&cx, responder, async move { state.initialize(req).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: AuthenticateRequest, responder, cx| {
                    let state = state.clone();
                    let cx_for_call = cx.clone();
                    spawn_response(&cx, responder, async move { state.authenticate(req, &cx_for_call).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: NewSessionRequest, responder, cx| {
                    let state = state.clone();
                    let cx_for_call = cx.clone();
                    spawn_response(&cx, responder, async move { state.new_session(req, &cx_for_call).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: ListSessionsRequest, responder, cx| {
                    let state = state.clone();
                    spawn_response(&cx, responder, async move { Ok(state.list_sessions(&req)) })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: LoadSessionRequest, responder, cx| {
                    let state = state.clone();
                    let cx_for_call = cx.clone();
                    spawn_response(&cx, responder, async move { state.load_session(req, &cx_for_call).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: PromptRequest, responder, cx| {
                    let state = state.clone();
                    cx.spawn(async move {
                        state.route_prompt(req, responder).await;
                        Ok(())
                    })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: SetSessionConfigOptionRequest, responder, cx| {
                    let state = state.clone();
                    cx.spawn(async move {
                        state.set_session_config_option(req, responder).await;
                        Ok(())
                    })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let state = state.clone();
                async move |req: PromptSearchParams, responder, cx| {
                    let state = state.clone();
                    spawn_response(&cx, responder, async move { state.search_prompts(&req) })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let state = state.clone();
                async move |notif: CancelNotification, _cx| {
                    let _ = state.cancel(notif).await;
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                async move |req: McpRequest, _cx| {
                    let _ = state.on_mcp_request(req).await;
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
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
