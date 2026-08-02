use super::common::*;
use acp_utils::{
    notifications::{
        BrowserAuthorizationParams, McpNotification, McpServerAuthCapability, McpServerStatus, McpServerStatusEntry,
    },
    testing::test_connection,
};
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn oauth_browser_prompt_is_rendered_inline_in_settings_overlay() -> TestResult {
    Box::pin(LocalSet::new().run_until(async {
        let mut renderer = open_settings(&[], (TEST_WIDTH, 40)).await?;
        renderer.on_mcp_notification(McpNotification::ServerStatus {
            servers: vec![oauth_server_status("linear", McpServerStatus::NeedsOAuth)],
        })?;

        press(&mut renderer, Enter).await?;
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_browser_authorization(&cx).await;
        renderer.on_browser_authorization_request(
            browser_authorization_params("linear", "Authorize linear?", "https://linear.app/oauth"),
            responder,
        )?;

        assert_buffer_contains(renderer.writer(), "Configuration");
        assert_buffer_contains(renderer.writer(), "Open browser to authorize linear MCP access");
        assert_buffer_contains(renderer.writer(), "linear.app");
        assert_buffer_contains(renderer.writer(), "Copy Link");
        assert!(!renderer.needs_mouse_capture(), "settings URL prompt should allow terminal text selection");

        press(&mut renderer, Esc).await?;
        let response = rx.await.expect("browser authorization prompt should be answered");
        assert!(!response.proceed, "Esc should cancel the browser flow");
        assert_buffer_contains(renderer.writer(), "Configuration");
        Ok(())
    }))
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn oauth_browser_completion_clears_settings_prompt() -> TestResult {
    Box::pin(LocalSet::new().run_until(async {
        let mut renderer = open_settings(&[], (TEST_WIDTH, 40)).await?;
        renderer.on_mcp_notification(McpNotification::ServerStatus {
            servers: vec![oauth_server_status("linear", McpServerStatus::Authenticating)],
        })?;

        press(&mut renderer, Enter).await?;
        let (cx, mut peer) = test_connection().await;
        let (responder, rx) = peer.fake_browser_authorization(&cx).await;
        renderer.on_browser_authorization_request(
            browser_authorization_params("linear", "Authorize linear?", "https://linear.app/oauth"),
            responder,
        )?;

        assert_buffer_contains(renderer.writer(), "Open browser to authorize linear MCP access");

        renderer.on_mcp_notification(McpNotification::BrowserAuthorizationCompleted {
            server_name: "linear".to_string(),
        })?;

        let response = rx.await.expect("completion should answer the pending browser prompt");
        assert!(response.proceed, "completion should proceed with the browser flow");
        assert_buffer_contains(renderer.writer(), "Configuration");
        assert_buffer_not_contains(renderer.writer(), "Open browser to authorize linear MCP access");
        Ok(())
    }))
    .await
}

#[tokio::test(flavor = "current_thread")]
async fn conversation_browser_prompt_completes_when_settings_is_closed() -> TestResult {
    LocalSet::new()
        .run_until(async {
            let mut renderer = RendererTest::new().size((TEST_WIDTH, 40)).build()?;
            let (cx, mut peer) = acp_utils::testing::test_connection().await;
            let (responder, rx) = peer.fake_browser_authorization(&cx).await;
            renderer.on_browser_authorization_request(
                browser_authorization_params("github", "Authorize GitHub", "https://github.com/login/oauth"),
                responder,
            )?;
            assert_buffer_contains(renderer.writer(), "Authorize GitHub");

            renderer.on_mcp_notification(McpNotification::BrowserAuthorizationCompleted {
                server_name: "github".to_string(),
            })?;

            let response = rx.await.expect("completion should answer the pending browser prompt");
            assert!(response.proceed);
            assert_buffer_contains(renderer.writer(), "github finished the browser flow");
            Ok(())
        })
        .await
}

fn browser_authorization_params(
    server_name: impl Into<String>,
    message: impl Into<String>,
    url: impl Into<String>,
) -> BrowserAuthorizationParams {
    BrowserAuthorizationParams { server_name: server_name.into(), message: message.into(), url: url.into() }
}

fn oauth_server_status(name: &str, status: McpServerStatus) -> McpServerStatusEntry {
    McpServerStatusEntry::new(name, status).with_auth_capability(McpServerAuthCapability::OAuth)
}
