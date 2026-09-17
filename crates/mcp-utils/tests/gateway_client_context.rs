use axum::{Json, Router, extract::State, routing::post};
use futures::StreamExt;
use mcp_utils::{
    client::{
        CallToolOptions, McpConfig, McpManager, McpTransport, RuntimeMcpServer, RuntimeMcpTransport, ToolCallEvent,
        ToolFilter, ToolMatcher, ToolRoute, call_tool,
    },
    request_context::{AgentIdentity, GatewayRequestContext, TOOL_CONTEXT_KEY},
};
use rmcp::model::RequestMetaObject;
use serde_json::{Map, Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::{Mutex, mpsc};
use utils::variables::Vars;

#[tokio::test]
async fn gateway_discovery_pages_calls_and_reconnections_keep_runtime_context() {
    let state = Arc::new(Mutex::new(Vec::<Value>::new()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let app = Router::new().route("/mcp", post(endpoint)).with_state(state.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let identity = AgentIdentity::new();
    let filter = ToolFilter { allow: vec![ToolMatcher::name("vm__coding__read_*")], deny: vec![] };
    let (tx, _rx) = mpsc::channel(64);
    let mut manager = McpManager::new(tx, None).with_identity(identity).with_tool_filter(filter.clone());
    for _ in 0..2 {
        let configured = McpConfig::from_json(
            &json!({"servers":{"vm":{
                "type":"http", "url":url, "aetherGateway":true,
                "tools":{"allow":["coding__*"]}, "deferTools":{"include":["coding__read_two"]}
            }}})
            .to_string(),
        )
        .unwrap()
        .into_servers(&Vars::new())
        .unwrap()
        .remove(0);
        let McpTransport::Http(http) = configured.transport else { panic!("expected HTTP") };
        manager
            .add_mcps(vec![
                RuntimeMcpServer::new("vm", RuntimeMcpTransport::Http(http), configured.tool_exposure)
                    .with_tools(configured.tools)
                    .with_aether_gateway(true),
            ])
            .await
            .unwrap();
        assert_eq!(
            manager.tool_definitions().iter().map(|tool| tool.name.as_str()).collect::<Vec<_>>(),
            ["vm__coding__read_one"]
        );
        let snapshot = manager.snapshot();
        let (client, params) = snapshot
            .resolve(ToolRoute::Deferred { server: "vm".into(), tool: "coding__read_two".into() }, Map::default())
            .unwrap();
        let mut meta = RequestMetaObject::default();
        meta.insert("traceparent".into(), json!("trace"));
        meta.insert(TOOL_CONTEXT_KEY.into(), json!({"forged":true}));
        let events = call_tool(
            client,
            params,
            CallToolOptions { timeout: Duration::from_secs(30), meta: Some(meta), ..Default::default() },
        )
        .collect::<Vec<_>>()
        .await;
        assert!(matches!(events.last(), Some(ToolCallEvent::Complete(Ok(_)))));
        manager.shutdown_server("vm").await.unwrap();
    }
    let requests = state.lock().await;
    let operations = requests
        .iter()
        .filter(|request| matches!(request["method"].as_str(), Some("tools/list" | "tools/call")))
        .collect::<Vec<_>>();
    assert_eq!(operations.len(), 6);
    for request in operations {
        let context: GatewayRequestContext =
            serde_json::from_value(request["params"]["_meta"][TOOL_CONTEXT_KEY].clone()).unwrap();
        assert_eq!(context.identity, identity);
        assert_eq!(context.agent_tools, filter);
        assert_eq!(context.server_alias, "vm");
        assert_eq!(context.server_tools.allow, [ToolMatcher::name("coding__*")]);
        if request["method"] == "tools/call" {
            assert_eq!(request["params"]["_meta"]["traceparent"], "trace");
        }
    }
    drop(requests);
    manager.shutdown().await;
    server.abort();
}

async fn endpoint(State(state): State<Arc<Mutex<Vec<Value>>>>, Json(request): Json<Value>) -> Json<Value> {
    state.lock().await.push(request.clone());
    let result = match request["method"].as_str() {
        Some("tools/list") if request["params"]["cursor"].is_null() => json!({"tools":[
            {"name":"coding__read_one", "inputSchema":{"type":"object"}},
            {"name":"review__review", "inputSchema":{"type":"object"}}
        ], "nextCursor":"second"}),
        Some("tools/list") => json!({"tools":[{"name":"coding__read_two", "inputSchema":{"type":"object"}}]}),
        Some("tools/call") => json!({"content":[], "structuredContent":{"ok":true}}),
        _ => {
            json!({"resultType":"complete", "supportedVersions":["2026-07-28"], "capabilities":{"tools":{}}, "ttlMs":0, "cacheScope":"private"})
        }
    };
    Json(json!({"jsonrpc":"2.0", "id":request["id"], "result":result}))
}
