use aether_tool_server::{HttpOptions, RemoteConfig, RemoteToolRuntime, serve};
use futures::StreamExt;
use mcp_utils::tool_policy::ToolFilter;
use mcp_utils::{
    client::{CallToolOptions, McpClient, McpConfig, ToolCallEvent, call_tool, client_capabilities},
    request_context::{AgentIdentity, GatewayRequestContext},
    testing::ElicitationScript,
};
use rmcp::{
    ClientLifecycleMode, RoleClient,
    model::{CallToolRequestParams, CallToolResult, ClientConfig, ElicitResult, Implementation, ProtocolVersion},
    serve_client_with_lifecycle,
    service::RunningService,
    transport::StreamableHttpClientTransport,
};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use utils::variables::Vars;

#[tokio::test]
async fn foreground_bash_composes_remote_review_without_restarting_shell() {
    let test = RemoteTest::start().await;
    let binary = env!("CARGO_BIN_EXE_aether-tool-server");
    let command = format!(
        "set -e; printf start >> sentinel; '{binary}' mcp review review_artifact --json '{{\"source\":{{\"type\":\"file\",\"path\":\"question.md\"}}}}'; printf finish >> sentinel"
    );
    let result = test.call("coding__bash", json!({"command":command})).await;
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    assert_eq!(std::fs::read_to_string(test.root.path().join("sentinel")).unwrap(), "startfinish", "{result:?}");
    assert_eq!(test.script.captured().len(), 1, "{result:?}");
    let request = serde_json::to_string(&test.script.captured()[0].request).unwrap();
    assert!(request.contains("Remote-only question"), "{request}");
    test.shutdown().await;
}

#[tokio::test]
async fn remote_cli_discovery_lists_only_deferred_tools() {
    let test = RemoteTest::start().await;
    let binary = env!("CARGO_BIN_EXE_aether-tool-server");
    let command = format!("'{binary}' mcp --help; '{binary}' mcp coding --help");
    let result = test.call("coding__bash", json!({"command":command})).await;
    let output = result.structured_content.unwrap();
    let output = output.to_string();
    assert!(output.contains("review"));
    assert!(!output.contains("subagents"));
    assert!(!output.contains("  read_file "));
    test.shutdown().await;
}

#[tokio::test]
async fn background_bash_composes_review_through_task_input() {
    let test = RemoteTest::start().await;
    let binary = env!("CARGO_BIN_EXE_aether-tool-server");
    let command = format!(
        "set -e; printf start >> sentinel; '{binary}' mcp review review_artifact --json '{{\"source\":{{\"type\":\"file\",\"path\":\"question.md\"}}}}'; printf finish >> sentinel"
    );
    let result = test.call("coding__bash", json!({"command":command,"runInBackground":true})).await;
    assert_eq!(result.structured_content.unwrap()["exitCode"], 0);
    assert_eq!(std::fs::read_to_string(test.root.path().join("sentinel")).unwrap(), "startfinish");
    assert_eq!(test.script.captured().len(), 1);
    test.shutdown().await;
}

#[tokio::test]
async fn direct_permissions_reject_decline_and_allow_approved_mutations() {
    let test = RemoteTest::configured(
        "always-ask",
        vec![json!({"action":"decline"}), json!({"action":"accept","content":{"decision":"allow"}})],
    )
    .await;
    let args = json!({"filePath":"created.txt","content":"approved"});
    assert!(test.call("coding__write_file", args.clone()).await.is_error.unwrap_or(false));
    assert!(!test.root.path().join("created.txt").exists());
    assert!(!test.call("coding__write_file", args).await.is_error.unwrap_or(false));
    assert_eq!(std::fs::read_to_string(test.root.path().join("created.txt")).unwrap(), "approved");
    test.shutdown().await;
}

#[tokio::test]
async fn nested_permission_pauses_the_same_foreground_shell() {
    let test = RemoteTest::configured(
        "always-ask",
        vec![
            json!({"action":"accept","content":{"decision":"allow"}}),
            json!({"action":"accept","content":{"decision":"allow"}}),
        ],
    )
    .await;
    let binary = env!("CARGO_BIN_EXE_aether-tool-server");
    let command = format!(
        "set -e; printf start >> sentinel; '{binary}' mcp coding write_file --json '{{\"filePath\":\"created.txt\",\"content\":\"nested\"}}'; printf finish >> sentinel"
    );
    let result = test.call("coding__bash", json!({"command":command})).await;
    assert_eq!(result.structured_content.unwrap()["exitCode"], 0);
    assert_eq!(std::fs::read_to_string(test.root.path().join("sentinel")).unwrap(), "startfinish");
    assert_eq!(std::fs::read_to_string(test.root.path().join("created.txt")).unwrap(), "nested");
    assert_eq!(test.script.captured().len(), 2);
    test.shutdown().await;
}

#[tokio::test]
async fn background_task_ownership_policy_and_cancellation_are_enforced() {
    use rmcp::model::{CallToolResponse, CancelTaskParams, GetTaskParams, InputResponses, UpdateTaskParams};
    let test = RemoteTest::start().await;
    let binary = env!("CARGO_BIN_EXE_aether-tool-server");
    let mut request = CallToolRequestParams::new("coding__bash").with_arguments(json!({"command":format!("'{binary}' mcp review review_artifact --json '{{\"source\":{{\"type\":\"file\",\"path\":\"question.md\"}}}}'; printf forbidden > sentinel"),"runInBackground":true}).as_object().unwrap().clone());
    request.meta = Some(test.metadata());
    let CallToolResponse::Task(created) = test.client.call_tool_once(request).await.unwrap() else {
        panic!("expected task");
    };
    let id = created.task.task_id;
    let mut get = GetTaskParams::new(id.clone());
    get.meta = Some(test.metadata());
    loop {
        let task = test.client.get_task(get.clone()).await.unwrap();
        if task.task.task.status == rmcp::model::TaskStatus::InputRequired {
            break;
        }
        tokio::task::yield_now().await;
    }
    let mut stranger = test.policy.clone();
    stranger.identity = AgentIdentity::new();
    let mut restricted = test.policy.clone();
    restricted.agent_tools = serde_json::from_value(json!({"deny":["vm__coding__bash"]})).unwrap();
    for policy in [stranger, restricted] {
        let mut meta = test.metadata();
        policy.merge_into(&mut meta).unwrap();
        let mut denied = get.clone();
        denied.meta = Some(meta.clone());
        assert!(test.client.get_task(denied).await.is_err());
        let mut cancel = CancelTaskParams::new(id.clone());
        cancel.meta = Some(meta.clone());
        assert!(test.client.cancel_task(cancel).await.is_err());
        let mut update = UpdateTaskParams::new(id.clone(), InputResponses::new());
        update.meta = Some(meta);
        assert!(test.client.update_task(update).await.is_err());
    }
    let mut cancel = CancelTaskParams::new(id);
    cancel.meta = Some(test.metadata());
    test.client.cancel_task(cancel).await.unwrap();
    loop {
        let task = test.client.get_task(get.clone()).await.unwrap();
        if task.task.task.status.is_terminal() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(!test.root.path().join("sentinel").exists());
    test.shutdown().await;
}

struct RemoteTest {
    root: tempfile::TempDir,
    runtime: RemoteToolRuntime,
    client: Arc<RunningService<RoleClient, McpClient>>,
    script: ElicitationScript,
    policy: GatewayRequestContext,
    cancellation: CancellationToken,
    server: tokio::task::JoinHandle<Result<(), aether_tool_server::RemoteError>>,
}

impl RemoteTest {
    async fn start() -> Self {
        Self::configured("always-allow", vec![json!({"action":"accept","content":{"decision":"approved"}})]).await
    }

    async fn configured(permission: &str, responses: Vec<Value>) -> Self {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("question.md"), "# Remote-only question").unwrap();
        let config = McpConfig::from_json(
            &json!({"servers":{"coding":{"type":"in-memory","args":["--disable-lsp", "--permission-mode", permission]},"review":{"type":"in-memory"}}}).to_string(),
        )
        .unwrap();
        let runtime =
            RemoteToolRuntime::new(RemoteConfig::new(root.path(), config, &Vars::new()).unwrap()).await.unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = CancellationToken::new();
        let server = tokio::spawn(serve(listener, runtime.clone(), HttpOptions::default(), cancellation.clone()));
        let (events, receiver) = mpsc::channel(16);
        let responses = responses.into_iter().map(|response| serde_json::from_value::<ElicitResult>(response).unwrap());
        let script = ElicitationScript::spawn(receiver, responses);
        let policy = GatewayRequestContext {
            identity: AgentIdentity::new(),
            server_alias: "vm".into(),
            agent_tools: ToolFilter::default(),
            server_tools: ToolFilter::default(),
            defer_tools: serde_json::from_value(json!({"include":["review__*", "coding__write_file"]})).unwrap(),
            execution_task: None,
        };
        let handler = McpClient::new(
            ClientConfig::new(client_capabilities(), Implementation::new("test", "1")),
            "vm".into(),
            events,
        )
        .with_request_context(policy.clone())
        .unwrap();
        let client = serve_client_with_lifecycle(
            handler,
            StreamableHttpClientTransport::from_uri(format!("http://{address}/mcp")),
            ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        )
        .await
        .unwrap();
        Self { root, runtime, client: Arc::new(client), policy, script, cancellation, server }
    }

    fn metadata(&self) -> rmcp::model::RequestMetaObject {
        let mut meta = rmcp::model::RequestMetaObject::default();
        self.policy.merge_into(&mut meta).unwrap();
        meta
    }

    async fn call(&self, tool: &str, arguments: Value) -> CallToolResult {
        let background = arguments["runInBackground"] == true;
        let request =
            CallToolRequestParams::new(tool.to_string()).with_arguments(arguments.as_object().unwrap().clone());
        let events = call_tool(
            self.client.clone(),
            request,
            CallToolOptions { timeout: Duration::from_secs(600), ..Default::default() },
        );
        tokio::pin!(events);
        while let Some(event) = events.next().await {
            match event {
                ToolCallEvent::Complete(result) | ToolCallEvent::TaskComplete { result, .. } => return result.unwrap(),
                ToolCallEvent::TaskCreated(_) => {
                    assert!(background, "foreground Bash must not become a background model event");
                }
                _ => {}
            }
        }
        panic!("missing tool result");
    }

    async fn shutdown(mut self) {
        drop(self.client);
        self.cancellation.cancel();
        self.server.await.unwrap().unwrap();
        let endpoint = self.runtime.gateway_endpoint().unwrap().to_path_buf();
        self.runtime.shutdown().await.unwrap();
        assert!(!endpoint.exists());
    }
}
