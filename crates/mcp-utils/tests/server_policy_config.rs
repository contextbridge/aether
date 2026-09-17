use mcp_utils::client::McpConfig;
use serde_json::json;
use utils::variables::Vars;

#[test]
fn server_policy_roundtrips_for_every_transport() {
    for server in [
        json!({"type":"stdio", "command":"example", "tools":{"allow":["read_*"],"deny":["read_secret"]}}),
        json!({"type":"http", "url":"http://localhost/mcp", "tools":{"allow":["coding__*"]}, "aetherGateway":true}),
        json!({"type":"in-memory", "tools":{"allow":[{"readOnly":true}]}}),
    ] {
        let config = McpConfig::from_json(&json!({"servers":{"example":server}}).to_string()).unwrap();
        let serialized = serde_json::to_value(&config).unwrap();
        assert_eq!(serialized["servers"]["example"]["tools"], server["tools"]);
        if server.get("aetherGateway").is_some() {
            assert_eq!(serialized["servers"]["example"]["aetherGateway"], true);
        }
        config.into_servers(&Vars::new()).unwrap();
    }
}

#[test]
fn gateway_requires_http_transport() {
    let config =
        McpConfig::from_json(r#"{"servers":{"vm":{"type":"sse","url":"http://localhost/mcp","aetherGateway":true}}}"#);
    assert!(config.and_then(|config| config.into_servers(&Vars::new())).is_err());
}
