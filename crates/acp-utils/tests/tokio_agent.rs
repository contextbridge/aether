use acp_utils::client::TokioAcpAgent;
use agent_client_protocol::{Client, ConnectTo};
use std::path::Path;
use std::str::FromStr;
use tokio::task::LocalSet;

#[test]
fn parses_shell_command() {
    let agent = TokioAcpAgent::from_str("aether acp --foo bar").expect("parses");
    let config = agent.config();
    assert_eq!(config.command(), Path::new("aether"));
    assert_eq!(config.arguments(), &["acp", "--foo", "bar"]);
}

#[test]
fn parses_quoted_shell_command() {
    let agent = TokioAcpAgent::from_str(r#"python "my agent.py" --name "Test Agent""#).expect("parses");
    let config = agent.config();
    assert_eq!(config.command(), Path::new("python"));
    assert_eq!(config.arguments(), &["my agent.py", "--name", "Test Agent"]);
}

#[test]
fn parses_leading_environment_variables() {
    let agent = TokioAcpAgent::from_str("RUST_LOG=debug aether acp").expect("parses");
    let config = agent.config();
    assert_eq!(config.command(), Path::new("aether"));
    assert_eq!(config.arguments(), &["acp"]);
    assert_eq!(config.environment().get("RUST_LOG").map(String::as_str), Some("debug"));
}

#[test]
fn parses_json_process_agent_config() {
    let agent = TokioAcpAgent::from_str(
        r#"{"command":"/usr/bin/python","args":["agent.py","--verbose"],"env":{"RUST_LOG":"debug"}}"#,
    )
    .expect("parses");

    let config = agent.config();
    assert_eq!(config.command(), Path::new("/usr/bin/python"));
    assert_eq!(config.arguments(), &["agent.py", "--verbose"]);
    assert_eq!(config.environment().get("RUST_LOG").map(String::as_str), Some("debug"));
}

#[test]
fn rejects_empty_command() {
    assert!(TokioAcpAgent::from_str("").is_err());
    assert!(TokioAcpAgent::from_str("   ").is_err());
}

#[test]
fn rejects_legacy_mcp_server_json() {
    let stdio = r#"{"type":"stdio","name":"test-agent","command":"/usr/bin/python","args":["agent.py"],"env":[{"name":"RUST_LOG","value":"debug"}]}"#;
    assert!(TokioAcpAgent::from_str(stdio).is_err());

    let http = r#"{"type":"http","name":"remote","url":"https://example.com/agent","headers":[]}"#;
    assert!(TokioAcpAgent::from_str(http).is_err());

    let array_env = r#"{"command":"python","args":["agent.py"],"env":[{"name":"RUST_LOG","value":"debug"}]}"#;
    assert!(TokioAcpAgent::from_str(array_env).is_err());
}

#[tokio::test(flavor = "current_thread")]
async fn agent_exited_message_includes_stderr_and_status() {
    LocalSet::new()
        .run_until(async {
            let agent = TokioAcpAgent::from_str("/bin/ls /nonexistent-aether-test-path-12345").expect("parses");
            let result = ConnectTo::<Client>::connect_to(agent, Client.builder()).await;

            let err = result.expect_err("child exited with non-zero status");
            let msg = format!("{err}");
            assert!(msg.contains("exited"), "expected exit info in error: {msg}");
            assert!(msg.contains("No such file"), "expected stderr in error: {msg}");
        })
        .await;
}
