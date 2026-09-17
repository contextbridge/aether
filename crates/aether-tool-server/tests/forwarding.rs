use aether_tool_server::{HttpOptions, RemoteConfig, RemoteToolRuntime, serve};
use mcp_utils::{
    client::{McpConfig, client_capabilities},
    request_context::{AgentIdentity, GatewayRequestContext},
    server::mrtr::input_requests_supported,
    tool_exposure::ToolExposure,
    tool_policy::ToolFilter,
};
use rmcp::{
    ClientLifecycleMode, RoleServer, ServerHandler,
    model::*,
    serve_client_with_lifecycle,
    service::RequestContext,
    transport::{
        StreamableHttpClientTransport,
        streamable_http_server::{
            StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
        },
    },
};
use serde_json::json;
use std::sync::{Arc, Mutex};
use tokio_util::sync::CancellationToken;
use utils::variables::Vars;

#[tokio::test]
async fn third_party_mrtr_preserves_capabilities_errors_and_opaque_state() {
    let fake = FakeLinear::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backend_address = listener.local_addr().unwrap();
    let backend = fake.clone();
    let mut config = StreamableHttpServerConfig::default();
    config.legacy_session_mode = false;
    config.json_response = true;
    let service =
        StreamableHttpService::new(move || Ok(backend.clone()), Arc::new(NeverSessionManager::default()), config);
    let stop_backend = CancellationToken::new();
    let shutdown = stop_backend.clone();
    let backend_task = tokio::spawn(async move {
        axum::serve(listener, axum::Router::new().nest_service("/mcp", service))
            .with_graceful_shutdown(shutdown.cancelled_owned())
            .await
            .unwrap();
    });
    let root = tempfile::tempdir().unwrap();
    let config = McpConfig::from_json(
        &json!({"servers":{"linear":{"type":"http","url":format!("http://{backend_address}/mcp")}}}).to_string(),
    )
    .unwrap();
    let runtime = RemoteToolRuntime::new(RemoteConfig::new(root.path(), config, &Vars::new()).unwrap()).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancellation = CancellationToken::new();
    let server = tokio::spawn(serve(listener, runtime, HttpOptions::default(), cancellation.clone()));
    let client = serve_client_with_lifecycle(
        ClientConfig::new(client_capabilities(), Implementation::new("test", "1")),
        StreamableHttpClientTransport::from_uri(format!("http://{address}/mcp")),
        ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
    )
    .await
    .unwrap();
    let policy = GatewayRequestContext {
        identity: AgentIdentity::new(),
        server_alias: "vm".into(),
        agent_tools: ToolFilter::default(),
        server_tools: ToolFilter::default(),
        defer_tools: ToolExposure::default(),
        execution_task: None,
    };
    let mut meta = RequestMetaObject::default();
    policy.merge_into(&mut meta).unwrap();
    let mut request = CallToolRequestParams::new("linear__create")
        .with_arguments(json!({"title":"Remote issue"}).as_object().unwrap().clone());
    request.meta = Some(meta.clone());
    let response = client.call_tool_once(request.clone()).await.unwrap();
    let CallToolResponse::InputRequired(input) = response else {
        panic!("expected interactive forwarding: {response:?}");
    };
    assert_ne!(input.request_state.as_deref(), Some("opaque-backend-state"));
    request.request_state = input.request_state;
    request.input_responses = Some(InputResponses::from([("approve".into(), json!({"action":"accept"}))]));
    let mut wrong_owner = policy.clone();
    wrong_owner.identity = AgentIdentity::new();
    let mut wrong = request.clone();
    wrong_owner.merge_into(wrong.meta.as_mut().unwrap()).unwrap();
    assert!(client.call_tool_once(wrong).await.is_err());
    let mut wrong = request.clone();
    wrong.arguments = Some(json!({"title":"changed"}).as_object().unwrap().clone());
    assert!(client.call_tool_once(wrong).await.is_err());
    assert!(fake.issues.lock().unwrap().is_empty());
    let result = client.call_tool(request.clone()).await.unwrap();
    assert_eq!(result.structured_content, Some(json!({"title":"Remote issue"})));
    assert!(client.call_tool_once(request).await.is_err(), "continuation replay must not repeat mutation");
    assert_eq!(*fake.issues.lock().unwrap(), vec!["Remote issue"]);

    let mut error = CallToolRequestParams::new("linear__fail");
    error.meta = Some(meta);
    let error = client.call_tool(error).await.unwrap_err();
    assert!(
        matches!(error, rmcp::service::ServiceError::McpError(ref data) if data.code == ErrorCode::INVALID_PARAMS),
        "{error:?}"
    );
    client.cancel().await.unwrap();
    cancellation.cancel();
    server.await.unwrap().unwrap();
    stop_backend.cancel();
    backend_task.await.unwrap();
}

#[derive(Clone, Default)]
struct FakeLinear {
    issues: Arc<Mutex<Vec<String>>>,
}

#[allow(clippy::unused_async_trait_impl)]
impl ServerHandler for FakeLinear {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let schema = Arc::new(json!({"type":"object"}).as_object().unwrap().clone());
        Ok(ListToolsResult::with_all_items(vec![
            Tool::new("create", "Create an issue", schema.clone()),
            Tool::new("fail", "Reject input", schema),
        ]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        assert!(!context.meta.contains_key(mcp_utils::request_context::TOOL_CONTEXT_KEY));
        if request.name == "fail" {
            return Err(ErrorData::invalid_params("invalid issue", Some(json!({"field":"title"}))));
        }
        if request.input_responses.is_none() {
            let inputs = InputRequests::from([(
                "approve".into(),
                InputRequest::Elicitation(ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
                    meta: None,
                    message: "Create issue?".into(),
                    requested_schema: ElicitationSchema::new(std::collections::BTreeMap::new()),
                })),
            )]);
            if !input_requests_supported(context.client_capabilities().as_ref(), &inputs) {
                return Err(ErrorData::missing_required_client_capability(client_capabilities()));
            }
            return Ok(InputRequiredResult::new(Some(inputs), Some("opaque-backend-state".into())).into());
        }
        assert_eq!(request.request_state.as_deref(), Some("opaque-backend-state"));
        let title = request.arguments.unwrap()["title"].as_str().unwrap().to_string();
        self.issues.lock().unwrap().push(title.clone());
        Ok(CallToolResult::structured(json!({"title":title})).into())
    }
}
