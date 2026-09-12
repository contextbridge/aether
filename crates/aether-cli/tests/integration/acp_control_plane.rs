use acp_utils::notifications::AetherCapabilities;
use aether_auth::OAuthCredentialStorage;
use aether_cli::acp::testing::AcpTestHarness;
use agent_client_protocol::schema::{ProtocolVersion, v2 as acp};
use serde_json::json;
use tokio::task::LocalSet;

#[tokio::test(flavor = "current_thread")]
async fn initialize_exposes_only_v2_nested_capabilities() {
    LocalSet::new()
        .run_until(async {
            let harness = AcpTestHarness::start().await;
            let response = harness.client_cx.send_request(initialize()).block_task().await.unwrap();
            assert_eq!(response.protocol_version, ProtocolVersion::V2);
            assert_eq!(response.info.name, "Aether");
            let wire = serde_json::to_value(&response).unwrap();
            let session = response.capabilities.session.unwrap();
            assert!(session.prompt.as_ref().unwrap().embedded_context.is_some());
            assert!(session.mcp.as_ref().unwrap().stdio.is_some());
            assert!(session.mcp.as_ref().unwrap().http.is_some());
            let meta = AetherCapabilities::from_meta(session.meta.as_ref());
            assert!(meta.prompt_search && meta.session_preview && meta.workspace_move);
            assert_eq!(
                AetherCapabilities::from_meta(session.prompt.unwrap().meta.as_ref()),
                AetherCapabilities::default()
            );
            for key in ["loadSession", "list", "resume", "close", "sse"] {
                assert!(!wire.to_string().contains(&format!("\"{key}\":")));
            }
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn login_and_logout_publish_credential_and_config_state() {
    LocalSet::new()
        .run_until(async {
            let mut harness = AcpTestHarness::start().await;
            let session = harness.insert_agent_switching_session().await;
            harness.oauth_store.save("unrelated-provider", json!({"token": "preserved"})).await.unwrap();
            assert!(!harness.oauth_store.contains("codex"));
            harness.client_cx.send_request(acp::LoginAuthRequest::new("codex")).block_task().await.unwrap();
            assert!(harness.oauth_store.contains("codex"));
            let methods = harness.auth_updates.recv().await.unwrap().auth_methods;
            assert!(authenticated(&methods));
            let logged_in = next_config(&mut harness, session.session_id()).await;

            harness.client_cx.send_request(acp::LogoutAuthRequest::new()).block_task().await.unwrap();
            assert!(harness.oauth_store.load("codex").await.unwrap().is_none());
            assert!(harness.oauth_store.contains("unrelated-provider"));
            let methods = harness.auth_updates.recv().await.unwrap().auth_methods;
            assert!(!authenticated(&methods));
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
    LocalSet::new()
        .run_until(async {
            let harness = AcpTestHarness::start().await;
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

fn initialize() -> acp::InitializeRequest {
    acp::InitializeRequest::new(ProtocolVersion::V2, acp::Implementation::new("test", "1"))
}

fn authenticated(methods: &[acp::AuthMethod]) -> bool {
    methods.iter().any(|method| matches!(method, acp::AuthMethod::Agent(method) if method.method_id.0.as_ref() == "codex" && method.description.as_deref() == Some("authenticated")))
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
