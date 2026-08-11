use super::common::*;
use acp_utils::{
    notifications::{
        ElicitRequestParams, ElicitationAction, ElicitationParams, McpNotification, McpServerAuthCapability,
        McpServerStatus, McpServerStatusEntry,
    },
    testing::test_connection,
};
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn oauth_url_prompt_is_rendered_inline_in_settings_overlay() -> TestResult {
    Box::pin(LocalSet::new().run_until(async {
        let mut renderer = open_settings(&[], (TEST_WIDTH, 40)).await?;
        renderer.on_mcp_notification(McpNotification::ServerStatus {
            servers: vec![oauth_server_status("linear", McpServerStatus::NeedsOAuth)],
        })?;

        press(&mut renderer, Enter).await?;
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_elicitation(&cx).await;
        renderer.on_elicitation_request(
            url_elicitation_params("linear", "Authorize linear?", "aether-oauth", "https://linear.app/oauth"),
            responder,
        )?;

        assert_buffer_contains(renderer.writer(), "Configuration");
        assert_buffer_contains(renderer.writer(), "Open browser to authorize linear MCP access");
        assert_buffer_contains(renderer.writer(), "linear.app");
        assert_buffer_contains(renderer.writer(), "Copy Link");
        assert!(!renderer.needs_mouse_capture(), "settings URL prompt should allow terminal text selection");

        press(&mut renderer, Esc).await?;
        let response = rx.await.expect("URL elicitation should be answered");
        assert_eq!(response.action, ElicitationAction::Cancel);
        assert_buffer_contains(renderer.writer(), "Configuration");
        Ok(())
    }))
    .await
}

fn url_elicitation_params(
    server_name: impl Into<String>,
    message: impl Into<String>,
    elicitation_id: impl Into<String>,
    url: impl Into<String>,
) -> ElicitationParams {
    ElicitationParams {
        server_name: server_name.into(),
        request: ElicitRequestParams::UrlElicitationParams {
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
