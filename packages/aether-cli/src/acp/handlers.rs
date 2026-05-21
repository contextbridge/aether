use super::session_manager::SessionManager;
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
pub(crate) fn acp_agent_builder(
    manager: Arc<SessionManager>,
) -> Builder<Agent, impl HandleDispatchFrom<Client>, NullRun> {
    Agent
        .builder()
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: InitializeRequest, responder, cx| {
                    let mgr = mgr.clone();
                    spawn_task(&cx, responder, async move { mgr.initialize(req).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: AuthenticateRequest, responder, cx| {
                    let mgr = mgr.clone();
                    let cx_for_call = cx.clone();
                    spawn_task(&cx, responder, async move { mgr.authenticate(req, &cx_for_call).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: NewSessionRequest, responder, cx| {
                    let mgr = mgr.clone();
                    let cx_for_call = cx.clone();
                    spawn_task(&cx, responder, async move { mgr.new_session(req, &cx_for_call).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: ListSessionsRequest, responder, cx| {
                    let mgr = mgr.clone();
                    spawn_task(&cx, responder, async move { mgr.list_sessions(&req) })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: LoadSessionRequest, responder, cx| {
                    let mgr = mgr.clone();
                    let cx_for_call = cx.clone();
                    spawn_task(&cx, responder, async move { mgr.load_session(req, &cx_for_call).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: PromptRequest, responder, cx| {
                    let mgr = mgr.clone();
                    spawn_task(&cx, responder, async move { mgr.prompt(req).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: SetSessionConfigOptionRequest, responder, cx| {
                    let mgr = mgr.clone();
                    spawn_task(&cx, responder, async move { mgr.set_session_config_option(req).await })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_request(
            {
                let mgr = manager.clone();
                async move |req: PromptSearchParams, responder, cx| {
                    let mgr = mgr.clone();
                    spawn_task(&cx, responder, async move { mgr.search_prompts(&req) })
                }
            },
            acp::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let mgr = manager.clone();
                async move |notif: CancelNotification, _cx| {
                    let _ = mgr.cancel(notif).await;
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                async move |req: McpRequest, _cx| {
                    let _ = manager.on_mcp_request(req).await;
                    Ok(())
                }
            },
            acp::on_receive_notification!(),
        )
}

/// Run a request handler off the dispatcher's event loop.
///
/// The framework processes inbound messages on a single async task; awaiting
/// long-running work directly in a handler closure starves notifications like
/// `session/cancel`. Spawning the work and replying via the moved `Responder`
/// frees the dispatcher to deliver further messages immediately.
fn spawn_task<T, U>(cx: &ConnectionTo<Client>, responder: Responder<T>, future: U) -> Result<(), acp::Error>
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
