use aether_auth::{SigV4Credentials, SigV4Signers};
use aether_core::mcp::{
    McpConfig, McpServerConfig, ParseError, StdioServerConfig, ToolExposure, ToolFilter, ToolMatcher, http_transport,
};
use http::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use mcp_utils::client::{HttpAuth, OAuthClientRegistration, OAuthSettings, Transport};
use mcp_utils::model::{Tool, ToolAnnotations};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::tempdir;
use utils::variables::{VarError, Vars};

#[test]
fn from_json_accepts_mcp_servers_key() {
    let config = McpConfig::from_json(r#"{"mcpServers": {"alpha": {"type": "stdio", "command": "a"}}}"#).unwrap();

    assert_eq!(config.servers.keys().collect::<Vec<_>>(), ["alpha"]);
}

#[test]
fn from_json_defaults_missing_type_to_stdio() {
    let server =
        only_server(r#"{"mcpServers": {"devtools": {"command": "npx", "args": ["-y", "chrome-devtools-mcp"]}}}"#);

    let McpServerConfig::Stdio(StdioServerConfig { command, args, defer_tools, .. }) = server else {
        panic!("expected a stdio server, got {server:?}");
    };
    assert_eq!(command, "npx");
    assert_eq!(args, ["-y", "chrome-devtools-mcp"]);
    assert!(!defer_tools.defers_tools());
}

#[test]
fn from_json_rejects_unknown_server_types() {
    for server in [
        r#"{"type":"deferTools","servers":{}}"#,
        r#"{"type":"htp","url":"https://example.com"}"#,
        r#"{"type":"sse","url":"https://example.com"}"#,
        r#"{"type":"unknown","servers":{"bad":{"type":"in-memory"}}}"#,
    ] {
        let json = format!(r#"{{"servers":{{"bad":{server}}}}}"#);
        assert!(McpConfig::from_json(&json).is_err(), "unknown server type was accepted: {server}");
    }
}

#[test]
fn from_json_rejects_unknown_fields_for_all_transports() {
    for server in [
        r#"{"command":"x","direct_tool":["bash"]}"#,
        r#"{"type":"http","url":"https://example.com","direct_tool":["bash"]}"#,
        r#"{"type":"in-memory","direct_tool":["bash"]}"#,
        r#"{"command":"x","deferTools":true,"direct_tools":["bash"]}"#,
    ] {
        let json = format!(r#"{{"servers":{{"bad":{server}}}}}"#);
        assert!(McpConfig::from_json(&json).is_err(), "unknown field was accepted: {server}");
    }
}

#[test]
fn from_json_rejects_a_zero_oauth_callback_port() {
    let json = r#"{"servers":{"bad":{"type":"http","url":"https://example.com/mcp","oauth":{"clientId":"client","callbackPort":0}}}}"#;

    assert!(McpConfig::from_json(json).is_err());
}

#[test]
fn defer_tools_accepts_booleans_and_rules_for_all_transports() {
    let config = McpConfig::from_json(
        r#"{"servers":{"all":{"command":"a","deferTools":true},"stdio":{"command":"x","deferTools":{"include":["lsp_*"],"exclude":["lsp_rename"]}},"http":{"type":"http","url":"https://example.com","deferTools":{"exclude":["bash"]}},"memory":{"type":"in-memory","deferTools":{"include":["read"]}},"direct":{"type":"http","url":"https://example.com"}}}"#,
    )
    .unwrap();

    assert_eq!(config.servers["all"].defer_tools(), &ToolExposure::deferred_all());
    assert_eq!(config.servers["stdio"].defer_tools(), &deferred(&["lsp_*"], &["lsp_rename"]));
    assert_eq!(config.servers["http"].defer_tools(), &deferred(&[], &["bash"]));
    assert_eq!(config.servers["memory"].defer_tools(), &deferred(&["read"], &[]));
    assert_eq!(config.servers["direct"].defer_tools(), &ToolExposure::ModelVisible);
}

#[test]
fn defer_tools_rejects_unknown_rule_fields() {
    for rules in [r#"{"allow":["lsp_*"]}"#, r#"{"deny":["bash"]}"#, r#"{"include":["lsp_*"],"direct":["bash"]}"#] {
        let json = format!(r#"{{"servers":{{"coding":{{"command":"x","deferTools":{rules}}}}}}}"#);
        assert!(McpConfig::from_json(&json).is_err(), "unknown deferTools field was accepted: {rules}");
    }
}

#[test]
fn defer_tools_serializes_only_when_tools_are_deferred() {
    let config = McpConfig::from_json(
        r#"{"servers":{"coding":{"command":"x","deferTools":{"exclude":["bash","lsp_*"]}},"direct":{"command":"y","deferTools":false},"full":{"command":"z","deferTools":true}}}"#,
    )
    .unwrap();

    let value = serde_json::to_value(config).unwrap();

    assert_eq!(value["servers"]["coding"]["deferTools"], serde_json::json!({"exclude":["bash", "lsp_*"]}));
    assert!(value["servers"]["direct"].get("deferTools").is_none());
    assert_eq!(value["servers"]["full"]["deferTools"], serde_json::json!(true));
}

#[test]
fn deferred_tool_rules_partition_tools_with_exclude_winning() {
    let server = only_server(
        r#"{"servers":{"coding":{"command":"server","deferTools":{"include":["lsp_*","bash"],"exclude":["lsp_rename"]}}}}"#,
    );
    let exposure = server.defer_tools();

    assert!(exposure.is_deferred(&tool("lsp_hover")));
    assert!(!exposure.is_deferred(&tool("lsp_rename")));
    assert!(exposure.is_deferred(&tool("bash")));
    assert!(!exposure.is_deferred(&tool("read_file")));
}

#[test]
fn deferred_tool_rules_match_tool_annotations() {
    let server = only_server(r#"{"servers":{"coding":{"command":"x","deferTools":{"exclude":[{"readOnly":true}]}}}}"#);
    let mut read_only = tool("read_file");
    read_only.annotations = Some(ToolAnnotations::new().read_only(true));

    assert!(!server.defer_tools().is_deferred(&read_only));
    assert!(server.defer_tools().is_deferred(&tool("write_file")));
}

#[test]
fn defer_all_tools_defers_every_server_and_keeps_existing_rules() {
    let mut config = McpConfig::from_json(
        r#"{"servers":{"a":{"type":"stdio","command":"a"},"b":{"type":"http","url":"https://example.com"},"coding":{"command":"server","deferTools":{"exclude":["bash"]}}}}"#,
    )
    .unwrap();

    config.defer_all_tools();

    assert!(config.servers.values().all(|server| server.defer_tools().defers_tools()));
    let coding = config.servers["coding"].defer_tools();
    assert!(!coding.is_deferred(&tool("bash")));
    assert!(coding.is_deferred(&tool("read_file")));
}

#[test]
fn merging_no_configs_is_empty() {
    assert!(std::iter::empty::<McpConfig>().collect::<McpConfig>().is_empty());
}

#[test]
fn merging_keeps_disjoint_servers() {
    let merged: McpConfig = [
        config(r#"{"servers": {"alpha": {"type": "stdio", "command": "a"}}}"#),
        config(r#"{"servers": {"beta": {"type": "stdio", "command": "b"}}}"#),
    ]
    .into_iter()
    .collect();

    assert_eq!(merged.servers.keys().collect::<Vec<_>>(), ["alpha", "beta"]);
}

#[test]
fn merging_lets_the_last_config_win_including_its_exposure() {
    let first =
        config(r#"{"servers":{"coding":{"type":"stdio","command":"first","deferTools":{"exclude":["bash"]}}}}"#);
    let second = config(r#"{"servers":{"coding":{"type":"stdio","command":"second"}}}"#);

    assert_eq!(coding([first.clone(), second.clone()]), command("second", ToolExposure::ModelVisible));
    assert_eq!(coding([second, first]), command("first", deferred(&[], &["bash"])));
}

#[test]
fn a_later_config_can_make_a_deferred_server_model_visible() {
    let mut lazy = config(r#"{"servers":{"coding":{"type":"stdio","command":"lazy"}}}"#);
    lazy.defer_all_tools();
    let visible = config(r#"{"servers":{"coding":{"type":"stdio","command":"visible","deferTools":false}}}"#);

    assert_eq!(coding([lazy, visible]), command("visible", ToolExposure::ModelVisible));
}

#[test]
fn reading_a_config_file_reports_read_and_parse_errors() {
    let dir = tempdir().unwrap();
    let invalid = write_config(dir.path(), "bad.json", "not valid json");

    assert!(matches!(McpConfig::from_json_file(dir.path().join("missing.json")), Err(ParseError::IoError(_))));
    assert!(matches!(McpConfig::from_json_file(invalid), Err(ParseError::JsonError(_))));
    assert!(matches!(McpConfig::from_json("{"), Err(ParseError::JsonError(_))));
}

#[test]
fn stdio_servers_expand_variables_and_run_in_the_given_directory() {
    let json = r#"{"servers":{"github":{"type":"stdio","command":"npx","args":["--root","${WORKSPACE}/src"],"env":{"GITHUB_TOKEN":"$GITHUB_TOKEN"}}}}"#;
    let vars = vars(&[("WORKSPACE", "/workspace"), ("GITHUB_TOKEN", "test_token")]);

    let Transport::Stdio { command, args, env, cwd } = transport(json, &vars) else { panic!("expected stdio") };

    assert_eq!(command, "npx");
    assert_eq!(args, ["--root", "/workspace/src"]);
    assert_eq!(env["GITHUB_TOKEN"], "test_token");
    assert_eq!(cwd.as_deref(), Some(Path::new("/cwd")));
}

#[test]
fn missing_variables_fail_resolution() {
    let json = r#"{"servers":{"test":{"type":"stdio","command":"$MISSING_VAR"}}}"#;

    let result = try_transport(json, &vars(&[]));

    assert!(matches!(result, Err(ParseError::VarError(VarError::NotFound(name))) if name == "MISSING_VAR"));
}

#[test]
fn http_servers_connect_to_their_expanded_url() {
    let vars = vars(&[("HOST", "localhost"), ("PORT", "8080")]);
    let json = r#"{"servers":{"remote":{"type":"http","url":"http://${HOST}:${PORT}/mcp"}}}"#;

    let Transport::Http { url, .. } = transport(json, &vars) else { panic!("expected http") };

    assert_eq!(url, "http://localhost:8080/mcp");
}

#[test]
fn http_headers_expand_variables_and_are_marked_sensitive() {
    let json = r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Authorization":"Bearer ${TOKEN}"}}}}"#;

    let Transport::Http { headers, .. } = transport(json, &vars(&[("TOKEN", "expanded-token")])) else {
        panic!("expected http")
    };

    assert_eq!(headers[AUTHORIZATION], "Bearer expanded-token");
    assert!(headers[AUTHORIZATION].is_sensitive());
}

#[test]
fn http_headers_keep_authorization_schemes_and_accept_any_header_casing() {
    let vars = vars(&[("TOKEN", "secret"), ("API_KEY", "custom-secret")]);
    for scheme in ["Sentry-Bearer", "Bearer", "bearer", "Basic", "Token"] {
        for header_name in ["Authorization", "authorization", "AUTHORIZATION"] {
            let json = format!(
                r#"{{"servers":{{"remote":{{"type":"http","url":"http://localhost/mcp","headers":{{"{header_name}":"{scheme} $TOKEN","X-API-Key":"$API_KEY"}}}}}}}}"#
            );

            let Transport::Http { headers, .. } = transport(&json, &vars) else { panic!("expected http") };

            assert_eq!(headers.get_all(AUTHORIZATION).iter().count(), 1);
            assert_eq!(headers[AUTHORIZATION], format!("{scheme} secret"));
            assert_eq!(headers["x-api-key"], "custom-secret");
        }
    }
}

#[test]
fn invalid_http_headers_fail_resolution() {
    let invalid_value =
        r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"X-Key":"bad\nvalue"}}}}"#;
    let invalid_name =
        r#"{"servers":{"weather":{"type":"http","url":"http://127.0.0.1:9000/mcp","headers":{"Bad Key":"value"}}}}"#;

    assert!(matches!(
        try_transport(invalid_value, &vars(&[])),
        Err(ParseError::InvalidHeaderValue { name, .. }) if name == "X-Key"
    ));
    assert!(matches!(
        try_transport(invalid_name, &vars(&[])),
        Err(ParseError::InvalidHeaderName { name, .. }) if name == "Bad Key"
    ));
}

#[test]
fn http_servers_sign_in_with_aether_client_metadata_by_default() {
    let oauth = http_oauth(remote(r#""headers":{}"#).unwrap()).expect("OAuth is offered");

    assert!(matches!(
        &oauth.registration,
        OAuthClientRegistration::ClientMetadata(url) if url == "https://aether-agent.io/oauth/client-metadata.json"
    ));
    assert_eq!(oauth.callback_port, 3118);
}

#[test]
fn http_servers_prefer_a_preregistered_client_id_over_client_metadata() {
    let oauth = r#""oauth":{"clientId":"registered","clientMetadataUrl":"https://client.example/oauth/client.json","callbackPort":4000}"#;

    let oauth = http_oauth(remote(oauth).unwrap()).expect("OAuth is offered");

    assert!(matches!(&oauth.registration, OAuthClientRegistration::PreRegistered(id) if id == "registered"));
    assert_eq!(oauth.callback_port, 4000);
}

#[test]
fn http_servers_with_an_authorization_header_never_sign_in() {
    let headers = HeaderMap::from_iter([(AUTHORIZATION, HeaderValue::from_static("Bearer static"))]);

    assert!(http_oauth(http_transport("https://example.com/mcp".to_string(), headers)).is_none());
    assert!(http_oauth(remote(r#""headers":{"Authorization":"Bearer static"}"#).unwrap()).is_none());
}

#[test]
fn oauth_settings_expand_variables() {
    let vars = vars(&[("CLIENT_ID", "registered"), ("CIMD_URL", "https://client.example/oauth/client.json")]);
    let oauth = |settings: &str| {
        let json = format!(
            r#"{{"servers":{{"linear":{{"type":"http","url":"https://example.com/mcp","oauth":{settings}}}}}}}"#
        );
        let Transport::Http { auth: HttpAuth::OAuth(oauth), .. } = transport(&json, &vars) else {
            panic!("expected HTTP transport with OAuth settings")
        };
        oauth
    };

    let registered = oauth(r#"{"clientId":"$CLIENT_ID","callbackPort":4000}"#);
    let metadata = oauth(r#"{"clientMetadataUrl":"$CIMD_URL"}"#);

    assert!(matches!(&registered.registration, OAuthClientRegistration::PreRegistered(id) if id == "registered"));
    assert_eq!(registered.callback_port, 4000);
    assert!(matches!(
        &metadata.registration,
        OAuthClientRegistration::ClientMetadata(url) if url == "https://client.example/oauth/client.json"
    ));
}

#[test]
fn sigv4_servers_sign_requests_for_the_configured_service_region_and_profile() {
    let vars = vars(&[("AWS_REGION", "us-west-2"), ("AWS_PROFILE", "gateway")]);
    let json = r#"{"servers":{"gateway":{"type":"http","url":"https://abc.execute-api.us-west-2.amazonaws.com/mcp","sigv4":{"service":"execute-api","region":"${AWS_REGION}","profile":"$AWS_PROFILE"}}}}"#;

    let Transport::Http { auth: HttpAuth::SigV4(signer), .. } = transport(json, &vars) else {
        panic!("expected HTTP transport with a SigV4 signer")
    };

    let config = signer.config();
    assert_eq!(config.service, "execute-api");
    assert_eq!(config.region, "us-west-2");
    assert!(
        matches!(&config.credentials, SigV4Credentials::DefaultChain { profile: Some(profile) } if profile == "gateway")
    );
}

#[test]
fn sigv4_servers_without_a_profile_use_the_default_credential_chain() {
    let json = r#"{"servers":{"gateway":{"type":"http","url":"https://example.com/mcp","sigv4":{"service":"lambda","region":"us-east-1"}}}}"#;

    let Transport::Http { auth: HttpAuth::SigV4(signer), .. } = transport(json, &vars(&[])) else {
        panic!("expected HTTP transport with a SigV4 signer")
    };

    assert!(matches!(signer.config().credentials, SigV4Credentials::DefaultChain { profile: None }));
}

#[test]
fn sigv4_requires_a_service_and_region_and_rejects_unknown_fields() {
    for sigv4 in [
        r#"{"region":"us-east-1"}"#,
        r#"{"service":"execute-api"}"#,
        r#"{"service":"execute-api","region":"us-east-1","servce":"lambda"}"#,
    ] {
        assert!(remote(&format!(r#""sigv4":{sigv4}"#)).is_err(), "invalid sigv4 config was accepted: {sigv4}");
    }
}

#[test]
fn a_server_sets_at_most_one_of_oauth_sigv4_or_an_authorization_header() {
    let oauth = r#""oauth":{"clientId":"id"}"#;
    let sigv4 = r#""sigv4":{"service":"execute-api","region":"us-east-1"}"#;
    let authorization = r#""headers":{"Authorization":"Bearer static"}"#;
    let lowercase_authorization = r#""headers":{"authorization":"Bearer static"}"#;

    for server in [
        format!("{oauth},{sigv4}"),
        format!("{authorization},{oauth}"),
        format!("{authorization},{sigv4}"),
        format!("{lowercase_authorization},{sigv4}"),
        format!("{authorization},{oauth},{sigv4}"),
    ] {
        let result = remote(&server);
        assert!(matches!(result, Err(ParseError::ConflictingAuth)), "{server} gave {result:?}");
    }
}

fn vars(pairs: &[(&str, &str)]) -> Vars {
    pairs.iter().fold(Vars::new().with_env_lookup(|_| None), |vars, (name, value)| vars.with(*name, *value))
}

fn only_server(json: &str) -> McpServerConfig {
    let mut servers = McpConfig::from_json(json).unwrap().servers;
    assert_eq!(servers.len(), 1);
    servers.pop_first().expect("one server").1
}

fn try_transport(json: &str, vars: &Vars) -> Result<Transport, ParseError> {
    match only_server(json) {
        McpServerConfig::Stdio(config) => config.into_transport(vars, Path::new("/cwd")),
        McpServerConfig::Remote(config) => config.into_transport(vars, &SigV4Signers::default()),
        McpServerConfig::InMemory(_) => panic!("in-memory servers have no transport"),
    }
}

fn transport(json: &str, vars: &Vars) -> Transport {
    try_transport(json, vars).unwrap()
}

fn config(json: &str) -> McpConfig {
    McpConfig::from_json(json).unwrap()
}

fn coding(configs: impl IntoIterator<Item = McpConfig>) -> (String, ToolExposure) {
    match configs.into_iter().collect::<McpConfig>().servers.remove("coding") {
        Some(McpServerConfig::Stdio(StdioServerConfig { command, defer_tools, .. })) => (command, defer_tools),
        other => panic!("expected a stdio server, got {other:?}"),
    }
}

fn command(command: &str, exposure: ToolExposure) -> (String, ToolExposure) {
    (command.to_string(), exposure)
}

fn http_oauth(transport: Transport) -> Option<OAuthSettings> {
    match transport {
        Transport::Http { auth: HttpAuth::OAuth(oauth), .. } => Some(oauth),
        Transport::Http { .. } => None,
        other => panic!("expected http, got {other:?}"),
    }
}

fn remote(server: &str) -> Result<Transport, ParseError> {
    let json = format!(r#"{{"servers":{{"remote":{{"type":"http","url":"https://example.com/mcp",{server}}}}}}}"#);
    let McpServerConfig::Remote(config) = McpConfig::from_json(&json)?.servers.pop_first().expect("one server").1
    else {
        panic!("expected a remote server")
    };
    config.into_transport(&vars(&[]), &SigV4Signers::default())
}

fn deferred(allow: &[&str], deny: &[&str]) -> ToolExposure {
    let matchers = |patterns: &[&str]| patterns.iter().copied().map(ToolMatcher::name).collect();
    ToolExposure::Deferred(ToolFilter { allow: matchers(allow), deny: matchers(deny) })
}

fn tool(name: &str) -> Tool {
    Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()))
}

fn write_config(dir: &Path, name: &str, json: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, json).unwrap();
    path
}
