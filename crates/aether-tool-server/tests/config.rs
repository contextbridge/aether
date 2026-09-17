use aether_tool_server::{RemoteConfig, RemoteToolRuntime};
use mcp_utils::client::{McpConfig, McpTransport};
use serde_json::json;
use utils::variables::Vars;

#[test]
fn remote_configuration_rejects_harness_factories_deferral_and_nested_gateways() {
    let root = tempfile::tempdir().unwrap();
    for server in [
        json!({"tasks":{"type":"in-memory"}}),
        json!({"unknown":{"type":"in-memory"}}),
        json!({"coding":{"type":"in-memory","deferTools":true}}),
        json!({"coding":{"type":"in-memory","input":{}}}),
        json!({"remote":{"type":"http","url":"http://localhost/mcp","aetherGateway":true}}),
        json!({"bad__alias":{"type":"stdio","command":"never-run"}}),
    ] {
        let config = McpConfig::from_json(&json!({"servers":server}).to_string()).unwrap();
        assert!(RemoteConfig::new(root.path(), config, &Vars::new()).is_err());
    }
}

#[test]
fn deployment_files_merge_and_workspace_variables_use_remote_root() {
    let root = tempfile::tempdir().unwrap();
    let first = root.path().join("first.json");
    let second = root.path().join("second.json");
    std::fs::write(&first, r#"{"servers":{"backend":{"command":"old"},"review":{"type":"in-memory"}}}"#).unwrap();
    std::fs::write(&second, r#"{"servers":{"backend":{"command":"new","args":["$WORKSPACE"]}}}"#).unwrap();
    let config = RemoteConfig::load(root.path(), &[first, second]).unwrap();
    assert_eq!(config.servers.len(), 2);
    let backend = config.servers.iter().find(|server| server.name == "backend").unwrap();
    let McpTransport::Stdio { command, args, .. } = &backend.transport else {
        panic!("expected stdio");
    };
    assert_eq!(command, "new");
    assert_eq!(args, &[root.path().canonicalize().unwrap().to_string_lossy()]);
    assert!(RemoteConfig::load(root.path(), &[]).is_err());
}

#[tokio::test]
async fn malformed_builtin_arguments_fail_startup() {
    let root = tempfile::tempdir().unwrap();
    for name in ["coding", "skills", "review"] {
        let config = McpConfig::from_json(
            &json!({"servers":{name:{"type":"in-memory","args":["--invalid-argument"]}}}).to_string(),
        )
        .unwrap();
        let config = RemoteConfig::new(root.path(), config, &Vars::new()).unwrap();
        assert!(RemoteToolRuntime::new(config).await.is_err());
    }
}
