use aether_auth::OAuthCredentialStorage;
use aether_cli::acp::testing::AcpTestHarness;
use agent_client_protocol::{schema::ProtocolVersion, v2::schema as acp};
use foxfire_acp::aether::{AetherCapabilities, AetherMeta};
use serde_json::json;

#[tokio::test(flavor = "current_thread")]
async fn initialize_exposes_only_v2_nested_capabilities() {
    AcpTestHarness::run(|harness| async move {
        let response = harness.initialize_response;
        assert_eq!(response.protocol_version, ProtocolVersion::V2);
        assert_eq!(response.info.name, "Aether");
        let wire = serde_json::to_value(&response).unwrap();
        let meta = AetherCapabilities::of(&response.capabilities);
        let every_extension = AetherCapabilities {
            prompt_search: true,
            session_preview: true,
            workspace_move: true,
            git_diff: true,
            workspace_status: true,
        };
        assert_eq!(meta, every_extension, "aether advertises every extension its clients gate on");
        assert!(response.capabilities.auth.as_ref().is_some_and(|auth| auth.status.is_some()));
        assert!(response.auth_methods.iter().all(|method| method.description().is_none()));
        let session = response.capabilities.session.unwrap();
        assert!(session.prompt.as_ref().unwrap().embedded_context.is_some());
        assert!(session.mcp.as_ref().unwrap().stdio.is_some());
        assert!(session.mcp.as_ref().unwrap().http.is_some());
        assert_eq!(AetherCapabilities::from_meta(session.prompt.unwrap().meta.as_ref()), AetherCapabilities::default());
        for key in ["loadSession", "list", "resume", "close", "sse"] {
            assert!(!wire.to_string().contains(&format!("\"{key}\":")));
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn login_and_logout_publish_credential_and_config_state() {
    AcpTestHarness::run(|mut harness| async move {
        let session = harness.insert_agent_switching_session().await;
        harness.oauth_store.save("unrelated-provider", json!({"token": "preserved"})).await.unwrap();
        assert!(!harness.oauth_store.contains("codex"));
        let codex = codex_label(&harness.initialize_response);
        let status = harness.client_cx.send_request(acp::AuthStatusRequest::new()).block_task().await.unwrap();
        assert!(!signed_in(&status.accounts, &codex));

        harness.client_cx.send_request(acp::LoginAuthRequest::new("codex")).block_task().await.unwrap();
        assert!(harness.oauth_store.contains("codex"));
        let update = harness.peer.next_auth_status_update().await;
        assert_eq!(update.state, acp::AuthState::Authenticated);
        assert!(signed_in(&update.accounts, &codex));
        let status = harness.client_cx.send_request(acp::AuthStatusRequest::new()).block_task().await.unwrap();
        assert!(signed_in(&status.accounts, &codex));
        let logged_in = next_config(&mut harness, session.session_id()).await;

        harness.client_cx.send_request(acp::LogoutAuthRequest::new()).block_task().await.unwrap();
        assert!(harness.oauth_store.load("codex").await.unwrap().is_none());
        assert!(harness.oauth_store.contains("unrelated-provider"));
        assert!(!signed_in(&harness.peer.next_auth_status_update().await.accounts, &codex));
        let logged_out = next_config(&mut harness, session.session_id()).await;
        assert_ne!(logged_in, logged_out, "credential state changes the model login hints");
        let wire = serde_json::to_value(logged_out).unwrap();
        assert!(wire.to_string().contains("Needs login"));
        assert!(
            wire.as_array()
                .unwrap()
                .iter()
                .all(|option| option.get("configId").is_some() && option.get("id").is_none())
        );

        harness.client_cx.send_request(acp::LogoutAuthRequest::new()).block_task().await.unwrap();
        assert!(!harness.oauth_store.contains("codex"), "logout is idempotent");
        assert!(harness.client_cx.send_request(acp::LoginAuthRequest::new("unknown")).block_task().await.is_err());
    })
    .await;
}

#[test]
fn config_values_are_tagged_and_unknown_categories_round_trip() {
    let id = acp::SetSessionConfigOptionRequest::new("session", "model", "model-id");
    let wire = serde_json::to_value(id).unwrap();
    assert_eq!(wire["configId"], "model");
    assert_eq!(wire["type"], "id");
    assert_eq!(wire["value"], "model-id");
    let boolean = acp::SetSessionConfigOptionRequest::new(
        "session",
        "flag",
        acp::SessionConfigOptionValue::Boolean { value: true },
    );
    let wire = serde_json::to_value(boolean).unwrap();
    assert_eq!(wire["type"], "boolean");
    assert_eq!(wire["value"], true);
    let wire = json!({"configId": "custom", "name": "Custom", "category": "future_category", "type": "select", "currentValue": "one", "options": []});
    let option: acp::SessionConfigOption = serde_json::from_value(wire).unwrap();
    assert_eq!(serde_json::to_value(option).unwrap()["category"], "future_category");
    let group = acp::SessionConfigSelectGroup::new("provider", "Provider", vec![]);
    assert_eq!(group.group_id.0.as_ref(), "provider");
    let wire = serde_json::to_value(group).unwrap();
    assert_eq!(wire["groupId"], "provider");
    assert!(wire.get("group").is_none());
}

#[test]
fn new_and_resume_default_omitted_lists_to_empty() {
    let omitted: acp::NewSessionRequest = serde_json::from_value(json!({"cwd": "/tmp"})).unwrap();
    let empty: acp::NewSessionRequest =
        serde_json::from_value(json!({"cwd": "/tmp", "additionalDirectories": [], "mcpServers": []})).unwrap();
    assert_eq!(omitted, empty);
    let omitted: acp::ResumeSessionRequest =
        serde_json::from_value(json!({"sessionId": "saved", "cwd": "/tmp"})).unwrap();
    let empty: acp::ResumeSessionRequest = serde_json::from_value(
        json!({"sessionId": "saved", "cwd": "/tmp", "additionalDirectories": [], "mcpServers": []}),
    )
    .unwrap();
    assert_eq!(omitted, empty);
}

#[tokio::test(flavor = "current_thread")]
async fn resume_accepts_empty_or_omitted_lists_and_rejects_unknown_cursors() {
    AcpTestHarness::run(|harness| async move {
        harness.append_stored_session("saved", "2026-05-01T00:00:00Z");
        for wire in [
            json!({"sessionId": "saved", "cwd": "/tmp"}),
            json!({"sessionId": "saved", "cwd": "/tmp", "additionalDirectories": [], "mcpServers": []}),
        ] {
            let request: acp::ResumeSessionRequest = serde_json::from_value(wire).unwrap();
            harness.client_cx.send_request(request).block_task().await.unwrap();
            harness.client_cx.send_request(acp::CloseSessionRequest::new("saved")).block_task().await.unwrap();
        }
        let request: acp::ResumeSessionRequest = serde_json::from_value(
            json!({"sessionId": "saved", "cwd": "/tmp", "replayFrom": {"type": "future_cursor"}}),
        )
        .unwrap();
        assert!(harness.client_cx.send_request(request).block_task().await.is_err());
    })
    .await;
}

fn codex_label(initialize: &acp::InitializeResponse) -> String {
    let codex = initialize.auth_methods.iter().find(|method| method.method_id().0.as_ref() == "codex");
    codex.expect("aether offers a codex login").name().to_owned()
}

fn signed_in(accounts: &[acp::AuthAccount], label: &str) -> bool {
    accounts.iter().any(|account| account.label == label)
}

async fn next_config(harness: &mut AcpTestHarness, session_id: &acp::SessionId) -> Vec<acp::SessionConfigOption> {
    loop {
        let notification = harness.peer.next_session_notification().await;
        if notification.session_id == *session_id
            && let acp::SessionUpdate::ConfigOptionUpdate(update) = notification.update
        {
            return update.config_options;
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn initialize_advertises_session_titles() {
    AcpTestHarness::run(|harness| async move {
        let session = harness.initialize_response.capabilities.session.unwrap();
        assert!(session.title.is_some());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn set_title_renames_a_stored_session_in_the_session_list() {
    AcpTestHarness::run(|harness| async move {
        harness.append_stored_session("saved", "2026-05-01T00:00:00Z");
        harness.append_stored_prompt("saved", "first prompt");

        let response = harness
            .client_cx
            .send_request(acp::SetSessionTitleRequest::new("saved", "  Release notes  "))
            .block_task()
            .await
            .unwrap();
        let listed = harness.client_cx.send_request(acp::ListSessionsRequest::new()).block_task().await.unwrap();

        assert_eq!(response.title, "Release notes");
        assert_eq!(listed.sessions[0].title.as_deref(), Some("Release notes"));
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn set_title_rejects_blank_titles_and_unknown_sessions() {
    AcpTestHarness::run(|harness| async move {
        harness.append_stored_session("saved", "2026-05-01T00:00:00Z");

        let blank = harness.client_cx.send_request(acp::SetSessionTitleRequest::new("saved", " ")).block_task().await;
        let unknown =
            harness.client_cx.send_request(acp::SetSessionTitleRequest::new("missing", "Title")).block_task().await;

        assert_eq!(blank.unwrap_err().code, acp::ErrorCode::InvalidParams);
        assert_eq!(unknown.unwrap_err().code, acp::ErrorCode::ResourceNotFound);
    })
    .await;
}
