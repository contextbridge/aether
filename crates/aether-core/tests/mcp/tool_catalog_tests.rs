use super::support::deferred_tools;
use aether_core::core::McpInstructions;
use aether_core::mcp::{McpCatalog, ToolAnnotationMatcher, ToolExposure, ToolFilter, ToolMatcher, mcp};
use aether_core::testing::ready_catalog;
use futures::StreamExt;
use llm::ToolDefinition;
use mcp_utils::client::{ToolCallOptions, Transport};
use mcp_utils::gateway::ServerSpec;
use mcp_utils::model::ToolAnnotations;
use mcp_utils::testing::{FakeAuthServer, FakeMcpServer, FakeTool, OAuthPrompt, args, elicitation_sink};
use serde_json::{Map, json};
use std::collections::HashMap;
use utils::mcp_status::{McpServerAuthCapability, McpServerStatus};

#[tokio::test]
async fn model_tools_are_namespaced_and_keep_their_metadata() {
    let server = FakeMcpServer::math()
        .with_tool(FakeTool::new("lookup").description("Looks up").annotations(ToolAnnotations::new().read_only(true)));
    let mcp = ready_catalog(mcp("/workspace").with_servers(vec![ServerSpec::in_process("math", server)])).await;

    let tools = mcp.tool_definitions();

    assert_eq!(names(&tools), ["math__add_numbers", "math__divide_numbers", "math__lookup"]);
    let lookup = &tools[2];
    assert_eq!(lookup.description, "Looks up");
    assert_eq!(lookup.server.as_deref(), Some("math"));
    assert_eq!(lookup.parameters, json!({"type": "object", "properties": {}}));
    assert_eq!(lookup.annotations.as_ref().and_then(|annotations| annotations.read_only_hint), Some(true));
}

#[tokio::test]
async fn deferred_tools_are_listed_apart_from_model_tools_and_not_callable_by_the_model() {
    let mcp = ready_catalog(mcp("/workspace").with_server(math("math"), deferring(&["divide_numbers"]))).await;

    let deferred_as_model = mcp.call_tool("math__divide_numbers", Map::new(), ToolCallOptions::default());

    assert_eq!(names(&mcp.tool_definitions()), ["math__add_numbers"]);
    assert_eq!(deferred_tools(&mcp).await, ["math__divide_numbers"]);
    assert!(deferred_as_model.is_none());
    assert!(mcp.statuses()[0].deferred_tools);
}

#[tokio::test]
async fn calls_reach_servers_whose_names_contain_the_delimiter() {
    let (math, math_v2) = (FakeMcpServer::math(), FakeMcpServer::math());
    let (math_state, math_v2_state) = (math.state(), math_v2.state());
    let servers = vec![ServerSpec::in_process("math", math), ServerSpec::in_process("math__v2", math_v2)];
    let mcp = ready_catalog(mcp("/workspace").with_servers(servers)).await;

    let sum = mcp.call_tool("math__v2__add_numbers", args(json!({"a": 2, "b": 3})), ToolCallOptions::default());

    assert_eq!(sum.unwrap().result().await.unwrap().structured_content, Some(json!({"sum": 5})));
    assert!(math_state.calls_for("add_numbers").is_empty());
    assert_eq!(math_v2_state.calls_for("add_numbers").len(), 1);
}

#[tokio::test]
async fn unknown_tools_are_not_offered() {
    let mcp = ready_catalog(mcp("/workspace").with_servers(vec![math("math")])).await;

    for name in ["math__missing", "other__add_numbers", "mathadd_numbers", "add_numbers"] {
        assert!(mcp.call_tool(name, Map::new(), ToolCallOptions::default()).is_none(), "{name} was offered");
    }
}

#[tokio::test]
async fn the_tool_filter_hides_model_and_deferred_tools() {
    let server =
        FakeMcpServer::math().with_tool(FakeTool::new("peek").annotations(ToolAnnotations::new().read_only(true)));
    let filter = ToolFilter {
        allow: Vec::new(),
        deny: vec![
            ToolMatcher::name("direct__divide_*"),
            ToolMatcher::annotations(ToolAnnotationMatcher {
                read_only: Some(true),
                ..ToolAnnotationMatcher::default()
            }),
        ],
    };
    let mcp = ready_catalog(
        mcp("/workspace")
            .with_servers(vec![ServerSpec::in_process("direct", server.clone())])
            .with_server(ServerSpec::in_process("lazy", server), ToolExposure::deferred_all())
            .with_tool_filter(filter),
    )
    .await;

    assert_eq!(names(&mcp.tool_definitions()), ["direct__add_numbers"]);
    assert_eq!(deferred_tools(&mcp).await, ["lazy__add_numbers", "lazy__divide_numbers"]);
    assert!(mcp.call_tool("direct__peek", Map::new(), ToolCallOptions::default()).is_none());
    let statuses = mcp.statuses();
    assert_eq!(statuses[0].status, McpServerStatus::Connected { tool_count: 3 });
    assert!(!statuses[0].deferred_tools && statuses[1].deferred_tools);
}

#[tokio::test]
async fn instructions_come_only_from_servers_offering_model_tools() {
    let server = FakeMcpServer::math().with_instructions("Use the math tools");
    let mcp = ready_catalog(
        mcp("/workspace")
            .with_servers(vec![ServerSpec::in_process("direct", server.clone())])
            .with_server(ServerSpec::in_process("lazy", server), ToolExposure::deferred_all()),
    )
    .await;

    let instructions = mcp.instructions();

    assert_eq!(instructions.servers, [("direct".to_string(), "Use the math tools".to_string())].into());
    assert!(instructions.progressive_discovery);
}

#[tokio::test]
async fn servers_whose_tools_are_all_filtered_out_give_no_instructions() {
    let server = FakeMcpServer::math().with_instructions("Use the math tools");
    let filter = ToolFilter { allow: vec![ToolMatcher::name("kept__*")], deny: Vec::new() };
    let servers = vec![ServerSpec::in_process("kept", server.clone()), ServerSpec::in_process("hidden", server)];
    let mcp = ready_catalog(mcp("/workspace").with_servers(servers).with_tool_filter(filter)).await;

    let instructions = mcp.instructions();

    assert_eq!(instructions.servers.keys().collect::<Vec<_>>(), ["kept"]);
}

#[tokio::test]
async fn deferred_servers_that_are_not_connected_need_no_discovery_instructions() {
    let mcp = ready_catalog(mcp("/workspace").with_server(missing_binary("lazy"), ToolExposure::deferred_all())).await;

    assert!(!mcp.instructions().progressive_discovery);
}

#[tokio::test]
async fn statuses_report_each_server_with_its_exposure_and_sign_in_support() {
    let auth = FakeAuthServer::start(FakeMcpServer::math()).await;
    let (sink, _host) = elicitation_sink();
    let mcp = ready_catalog(
        mcp("/workspace")
            .with_servers(vec![math("math"), auth.spec("linear")])
            .with_server(missing_binary("broken"), ToolExposure::deferred_all())
            .with_elicitation(sink),
    )
    .await;

    let statuses = mcp.statuses();

    let status = |name: &str| statuses.iter().find(|entry| entry.name == name).expect("a status for every server");
    assert_eq!(status("math").status, McpServerStatus::Connected { tool_count: 2 });
    assert_eq!(status("math").auth_capability, McpServerAuthCapability::Unavailable);
    assert!(!status("math").deferred_tools);
    assert_eq!(status("linear").status, McpServerStatus::NeedsOAuth);
    assert_eq!(status("linear").auth_capability, McpServerAuthCapability::OAuth);
    assert!(matches!(&status("broken").status, McpServerStatus::Failed { error } if error.contains("Failed to spawn")));
    assert_eq!(status("broken").auth_capability, McpServerAuthCapability::Unavailable);
    assert!(status("broken").deferred_tools);
}

#[tokio::test]
async fn signing_in_connects_a_server_with_its_configured_exposure() {
    let auth = FakeAuthServer::start(FakeMcpServer::math()).await;
    let (sink, mut host) = elicitation_sink();
    let mcp = ready_catalog(
        mcp("/workspace").with_server(auth.spec("linear"), deferring(&["divide_numbers"])).with_elicitation(sink),
    )
    .await;
    let signing_in = tokio::spawn({
        let mcp = mcp.clone();
        async move { mcp.authenticate("linear").await }
    });

    let prompt = OAuthPrompt::receive(&mut host).await;
    assert_eq!(mcp.statuses()[0].status, McpServerStatus::Authenticating);
    prompt.approve().await;
    signing_in.await.unwrap().expect("signing in succeeds");

    assert_eq!(mcp.statuses()[0].status, McpServerStatus::Connected { tool_count: 2 });
    assert_eq!(names(&mcp.tool_definitions()), ["linear__add_numbers"]);
    assert_eq!(deferred_tools(&mcp).await, ["linear__divide_numbers"]);
}

#[tokio::test]
async fn the_default_catalog_offers_nothing_and_never_changes() {
    let mcp = McpCatalog::default();

    assert!(mcp.tool_definitions().is_empty() && mcp.statuses().is_empty());
    assert_eq!(mcp.instructions(), McpInstructions::default());
    assert!(mcp.deferred_tools_socket().is_none());
    assert!(mcp.changes().next().await.is_none());
}

fn math(name: &str) -> ServerSpec {
    ServerSpec::in_process(name, FakeMcpServer::math())
}

fn missing_binary(name: &str) -> ServerSpec {
    let transport = Transport::Stdio {
        command: "aether-missing-mcp-server".to_string(),
        args: Vec::new(),
        env: HashMap::new(),
        cwd: None,
    };
    ServerSpec { name: name.to_string(), transport }
}

fn deferring(tools: &[&str]) -> ToolExposure {
    ToolExposure::Deferred(ToolFilter {
        allow: tools.iter().copied().map(ToolMatcher::name).collect(),
        deny: Vec::new(),
    })
}

fn names(tools: &[ToolDefinition]) -> Vec<&str> {
    tools.iter().map(|tool| tool.name.as_str()).collect()
}
