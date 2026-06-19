use super::common::*;
use acp_utils::{
    notifications::{
        CreateElicitationRequestParams, ElicitationAction, ElicitationParams, McpNotification, McpServerAuthCapability,
        McpServerStatus, McpServerStatusEntry, UrlElicitationCompleteParams,
    },
    testing::test_connection,
};
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn oauth_url_prompt_is_rendered_inline_in_settings_overlay() {
    Box::pin(LocalSet::new().run_until(async {
        let mut renderer = open_settings(&[], (TEST_WIDTH, 40)).await;
        renderer
            .on_mcp_notification(McpNotification::ServerStatus {
                servers: vec![oauth_server_status("linear", McpServerStatus::NeedsOAuth)],
            })
            .unwrap();

        press_enter(&mut renderer).await;
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        renderer
            .on_elicitation_request(
                url_elicitation_params("linear", "Authorize linear?", "aether-oauth", "https://linear.app/oauth"),
                responder,
            )
            .unwrap();

        assert_buffer_contains(renderer.writer(), "Configuration");
        assert_buffer_contains(renderer.writer(), "Open browser to authorize linear MCP access");
        assert_buffer_contains(renderer.writer(), "linear.app");
        assert_buffer_contains(renderer.writer(), "Copy Link");
        assert!(!renderer.needs_mouse_capture(), "settings URL prompt should allow terminal text selection");

        press_esc(&mut renderer).await;
        let response = rx.await.expect("URL elicitation should be answered");
        assert_eq!(response.action, ElicitationAction::Cancel);
        assert_buffer_contains(renderer.writer(), "Configuration");
    }))
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn oauth_url_completion_accepts_and_clears_settings_prompt() {
    Box::pin(LocalSet::new().run_until(async {
        let mut renderer = open_settings(&[], (TEST_WIDTH, 40)).await;
        renderer
            .on_mcp_notification(McpNotification::ServerStatus {
                servers: vec![oauth_server_status("linear", McpServerStatus::Authenticating)],
            })
            .unwrap();

        press_enter(&mut renderer).await;
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        renderer
            .on_elicitation_request(
                url_elicitation_params("linear", "Authorize linear?", "aether-oauth", "https://linear.app/oauth"),
                responder,
            )
            .unwrap();

        assert_buffer_contains(renderer.writer(), "Open browser to authorize linear MCP access");

        renderer
            .on_mcp_notification(McpNotification::UrlElicitationComplete(completion("linear", "aether-oauth")))
            .unwrap();

        let response = rx.await.expect("completion should answer URL elicitation");
        assert_eq!(response.action, ElicitationAction::Accept);
        assert_buffer_contains(renderer.writer(), "Configuration");
        assert_buffer_not_contains(renderer.writer(), "Open browser to authorize linear MCP access");
    }))
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_url_prompt_still_completes_when_settings_is_closed() {
    LocalSet::new()
        .run_until(async {
            let mut renderer = new_test_renderer((TEST_WIDTH, 40));
            let (cx, mut peer) = acp_utils::testing::test_connection().await;
            let (responder, rx) = peer.fake_elicitation(&cx).await;
            renderer
                .on_elicitation_request(
                    url_elicitation_params("github", "Authorize GitHub", "el-1", "https://github.com/login/oauth"),
                    responder,
                )
                .unwrap();
            assert_buffer_contains(renderer.writer(), "Authorize GitHub");

            renderer
                .on_mcp_notification(McpNotification::UrlElicitationComplete(completion("github", "el-1")))
                .unwrap();

            let response = rx.await.expect("completion should answer URL elicitation");
            assert_eq!(response.action, ElicitationAction::Accept);
            assert_buffer_contains(renderer.writer(), "github finished the browser flow");
        })
        .await;
}

fn url_elicitation_params(
    server_name: impl Into<String>,
    message: impl Into<String>,
    elicitation_id: impl Into<String>,
    url: impl Into<String>,
) -> ElicitationParams {
    ElicitationParams {
        server_name: server_name.into(),
        request: CreateElicitationRequestParams::UrlElicitationParams {
            meta: None,
            message: message.into(),
            url: url.into(),
            elicitation_id: elicitation_id.into(),
        },
    }
}

fn oauth_server_status(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_auth_capability(McpServerAuthCapability::OAuth)
}

fn completion(server_name: &str, elicitation_id: &str) -> UrlElicitationCompleteParams {
    UrlElicitationCompleteParams { server_name: server_name.to_string(), elicitation_id: elicitation_id.to_string() }
}
