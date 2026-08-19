use aether_auth::OAuthClientRegistration;
use mcp_utils::client::{McpConfig, McpHttpConfig, McpTransport};
use utils::variables::Vars;

#[tokio::test]
async fn http_oauth_defaults_to_aether_cimd_and_fixed_callback() {
    let config = parse_http_config(r#"{ "type": "http", "url": "https://example.com/mcp" }"#, &Vars::new());
    let oauth = config.resolved_oauth().expect("OAuth should be offered without an auth header");

    assert_eq!(
        oauth.client_registration,
        OAuthClientRegistration::ClientMetadata("https://aether-agent.io/oauth/client-metadata.json".to_string())
    );
    assert_eq!(oauth.callback_port.get(), 3118);
    assert_eq!(oauth.redirect_uri(), "http://localhost:3118/");
}

#[tokio::test]
async fn custom_cimd_expands_variables_and_preregistered_client_takes_priority() {
    let vars = Vars::new().with("CLIENT_ID", "registered").with("CIMD_URL", "https://client.example/oauth/client.json");
    let config = parse_http_config(
        r#"{
            "type": "http",
            "url": "https://example.com/mcp",
            "oauth": {
                "clientId": "$CLIENT_ID",
                "clientMetadataUrl": "$CIMD_URL",
                "callbackPort": 4000
            }
        }"#,
        &vars,
    );
    let oauth = config.resolved_oauth().expect("OAuth should be offered without an auth header");

    assert_eq!(oauth.client_registration, OAuthClientRegistration::PreRegistered("registered".to_string()));
    assert_eq!(oauth.callback_port.get(), 4000);
    assert_eq!(config.oauth.unwrap().client_metadata_url.as_deref(), Some("https://client.example/oauth/client.json"));
}

#[tokio::test]
async fn bearer_header_bypasses_oauth_defaults() {
    let config = parse_http_config(
        r#"{
            "type": "http",
            "url": "https://example.com/mcp",
            "headers": { "Authorization": "Bearer token" }
        }"#,
        &Vars::new(),
    );

    assert!(config.resolved_oauth().is_none());
}

fn parse_http_config(server: &str, vars: &Vars) -> McpHttpConfig {
    let json = format!(r#"{{ "servers": {{ "remote": {server} }} }}"#);
    let mut servers = McpConfig::from_json(&json).unwrap().into_servers(vars).unwrap();
    assert_eq!(servers.len(), 1);
    let server = servers.remove(0);
    let McpTransport::Http(config) = server.transport else { panic!("expected HTTP transport") };
    config
}
