use super::common::{CodingWorkspace, TestClient};
use mcp_servers::coding::CodingMcp;
use mcp_utils::request_context::{AgentIdentity, GatewayRequestContext};
use mcp_utils::{tool_exposure::ToolExposure, tool_policy::ToolFilter};
use rmcp::model::RequestMetaObject;
use serde_json::json;

#[tokio::test]
async fn reads_never_authorize_another_identity_to_edit_or_overwrite() {
    let workspace = CodingWorkspace::new().await.unwrap();
    workspace.write("file.txt", "original").unwrap();
    let a = context();
    let b = context();
    workspace.client.call_raw_with_meta("read_file", json!({"filePath":"file.txt"}), Some(a.clone())).await.unwrap();
    let denied = workspace
        .client
        .call_raw_with_meta("write_file", json!({"filePath":"file.txt","content":"wrong agent"}), Some(b.clone()))
        .await
        .unwrap();
    assert!(denied.is_error.unwrap_or(false));
    let denied = workspace
        .client
        .call_raw_with_meta(
            "edit_file",
            json!({"filePath":"file.txt","edits":[{"oldString":"original","newString":"wrong agent"}]}),
            Some(b),
        )
        .await
        .unwrap();
    assert!(denied.is_error.unwrap_or(false));
    assert_eq!(workspace.read("file.txt").unwrap(), "original");
    let allowed = workspace
        .client
        .call_raw_with_meta("write_file", json!({"filePath":"file.txt","content":"agent a"}), Some(a))
        .await
        .unwrap();
    assert!(!allowed.is_error.unwrap_or(false));
    assert_eq!(workspace.read("file.txt").unwrap(), "agent a");
}

#[tokio::test]
async fn each_identity_receives_its_own_first_rule_delivery() {
    let root = tempfile::tempdir().unwrap();
    let rules = root.path().join("rules");
    std::fs::create_dir(&rules).unwrap();
    std::fs::write(
        rules.join("rust.md"),
        "---\ndescription: Rust\npaths:\n  - '**/*.rs'\n---\nUnique rule reminder.\n",
    )
    .unwrap();
    std::fs::write(root.path().join("main.rs"), "fn main() {}\n").unwrap();
    let client =
        TestClient::start(|| CodingMcp::new().with_root_dir(root.path().to_path_buf()).with_rules_dirs(vec![rules]))
            .await
            .unwrap();
    for meta in [context(), context()] {
        for first in [true, false] {
            let result = client
                .call_raw_with_meta("read_file", json!({"filePath":"main.rs"}), Some(meta.clone()))
                .await
                .unwrap();
            assert_eq!(serde_json::to_string(&result).unwrap().contains("Unique rule reminder."), first);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn idle_agent_safety_state_expires_and_identity_capacity_rejects_new_agents() {
    let workspace = CodingWorkspace::new().await.unwrap();
    workspace.write("file.txt", "original").unwrap();
    let first = context();
    for meta in std::iter::once(first.clone()).chain((1..1024).map(|_| context())) {
        let result =
            workspace.client.call_raw_with_meta("read_file", json!({"filePath":"file.txt"}), Some(meta)).await.unwrap();
        assert!(!result.is_error.unwrap_or(false));
    }
    assert!(
        workspace
            .client
            .call_raw_with_meta("read_file", json!({"filePath":"file.txt"}), Some(context()))
            .await
            .is_err()
    );
    let permitted = workspace
        .client
        .call_raw_with_meta("write_file", json!({"filePath":"file.txt","content":"before expiry"}), Some(first.clone()))
        .await
        .unwrap();
    assert!(!permitted.is_error.unwrap_or(false));
    tokio::time::advance(std::time::Duration::from_secs(3601)).await;
    let denied = workspace
        .client
        .call_raw_with_meta("write_file", json!({"filePath":"file.txt","content":"after expiry"}), Some(first))
        .await
        .unwrap();
    assert!(denied.is_error.unwrap_or(false));
    assert_eq!(workspace.read("file.txt").unwrap(), "before expiry");
    let admitted = workspace
        .client
        .call_raw_with_meta("read_file", json!({"filePath":"file.txt"}), Some(context()))
        .await
        .unwrap();
    assert!(!admitted.is_error.unwrap_or(false));
}

#[tokio::test]
async fn failed_reads_do_not_authorize_later_overwrites() {
    let workspace = CodingWorkspace::new().await.unwrap();
    let meta = context();
    let result = workspace
        .client
        .call_raw_with_meta("read_file", json!({"filePath":"missing.txt"}), Some(meta.clone()))
        .await
        .unwrap();
    assert!(result.is_error.unwrap_or(false));
    workspace.write("missing.txt", "created externally").unwrap();
    let denied = workspace
        .client
        .call_raw_with_meta("write_file", json!({"filePath":"missing.txt","content":"overwrite"}), Some(meta.clone()))
        .await
        .unwrap();
    assert!(denied.is_error.unwrap_or(false));
    let allowed = workspace
        .client
        .call_raw_with_meta("write_file", json!({"filePath":"new.txt","content":"new"}), Some(meta))
        .await
        .unwrap();
    assert!(!allowed.is_error.unwrap_or(false));
    assert_eq!(workspace.read("missing.txt").unwrap(), "created externally");
    assert_eq!(workspace.read("new.txt").unwrap(), "new");
}

fn context() -> RequestMetaObject {
    let context = GatewayRequestContext {
        identity: AgentIdentity::new(),
        execution_task: None,
        server_alias: "vm".into(),
        agent_tools: ToolFilter::default(),
        server_tools: ToolFilter::default(),
        defer_tools: ToolExposure::default(),
    };
    let mut meta = RequestMetaObject::default();
    context.merge_into(&mut meta).unwrap();
    meta
}
