use aether_core::mcp::{McpConfig, McpSpawnError, mcp};
use clap::ValueEnum;
use mcp_servers::{Builtin, McpBuilderExt};
use tempfile::tempdir;

#[tokio::test]
async fn invalid_permission_mode_fails_spawn_instead_of_disabling_permission_prompts() {
    let error = spawn_error("coding", &["--permission-mode", "alwaysask"]);

    assert!(matches!(error, McpSpawnError::ServerFactory { ref name, .. } if name == "coding"));
    assert!(error.to_string().contains("alwaysask"), "error should name the bad value: {error}");
}

#[tokio::test]
async fn every_builtin_server_rejects_unknown_args() {
    for builtin in Builtin::value_variants() {
        let error = spawn_error(builtin.name(), &["--unknown-flag"]);

        assert!(matches!(error, McpSpawnError::ServerFactory { ref name, .. } if name == builtin.name()));
    }
}

#[test]
fn builtin_servers_parse_from_their_names() {
    for builtin in Builtin::value_variants() {
        assert_eq!(Builtin::from_str(builtin.name(), false).unwrap(), *builtin);
    }

    assert!(Builtin::from_str("nope", false).is_err());
}

fn spawn_error(server: &str, args: &[&str]) -> McpSpawnError {
    let root = tempdir().expect("create workspace");
    let config = serde_json::json!({ "servers": { server: { "type": "in-memory", "args": args } } });
    let config = McpConfig::from_json(&config.to_string()).expect("config parses");
    let builder = mcp(root.path()).with_builtin_servers().with_config(config);

    match builder.spawn() {
        Ok(_) => panic!("{server} should reject {args:?}"),
        Err(error) => error,
    }
}
