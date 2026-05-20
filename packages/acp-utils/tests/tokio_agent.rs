use acp_utils::client::TokioAcpAgent;
use agent_client_protocol::{Client, ConnectTo};
use std::path::Path;
use std::str::FromStr;
use tokio::task::LocalSet;

#[test]
fn parses_shell_command() {
    let agent = TokioAcpAgent::from_str("aether acp --foo bar").expect("parses");
    let server = agent.stdio();
    assert_eq!(server.command.as_path(), Path::new("aether"));
    assert_eq!(server.args.as_slice(), &["acp", "--foo", "bar"]);
}

#[test]
fn parses_quoted_shell_command() {
    let agent = TokioAcpAgent::from_str(r#"python "my agent.py" --name "Test Agent""#).expect("parses");
    let server = agent.stdio();
    assert_eq!(server.command.as_path(), Path::new("python"));
    assert_eq!(server.args.as_slice(), &["my agent.py", "--name", "Test Agent"]);
}

#[test]
fn parses_leading_environment_variables() {
    let agent = TokioAcpAgent::from_str("RUST_LOG=debug aether acp").expect("parses");
    let server = agent.stdio();
    assert_eq!(server.command.as_path(), Path::new("aether"));
    assert_eq!(server.args.as_slice(), &["acp"]);
    assert_eq!(server.env[0].name, "RUST_LOG");
    assert_eq!(server.env[0].value, "debug");
}

#[test]
fn parses_json_stdio_agent_config() {
    let agent = TokioAcpAgent::from_str(
        r#"{"type":"stdio","name":"test-agent","command":"/usr/bin/python","args":["agent.py","--verbose"],"env":[{"name":"RUST_LOG","value":"debug"}]}"#,
    )
    .expect("parses");

    let server = agent.stdio();
    assert_eq!(server.command.as_path(), Path::new("/usr/bin/python"));
    assert_eq!(server.args.as_slice(), &["agent.py", "--verbose"]);
    assert_eq!(server.env[0].name, "RUST_LOG");
    assert_eq!(server.env[0].value, "debug");
}

#[test]
fn rejects_empty_command() {
    assert!(TokioAcpAgent::from_str("").is_err());
    assert!(TokioAcpAgent::from_str("   ").is_err());
}

#[test]
fn rejects_non_stdio_transport_at_parse_time() {
    let json = r#"{"type":"http","name":"remote","url":"https://example.com/agent","headers":[]}"#;
    assert!(TokioAcpAgent::from_str(json).is_err());
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
