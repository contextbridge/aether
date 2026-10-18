use aether_auth::MemoryCredentialStore;
use aether_cli::headless::{CliEventKind, RunConfig, run::HeadlessRunner};
use aether_cli::output::OutputFormat;
use aether_core::agent_spec::AgentSpec;
use aether_core::events::{AgentEvent, MessageEvent, ToolEvent, TurnEvent, TurnOutcome};
use aether_core::mcp::McpConfig;
use aether_project::AgentCatalog;
use llm::LlmResponse;
use llm::testing::{FakeLlmProvider, llm_response};
use std::process::ExitCode;
use std::sync::Arc;
use utils::testing::TestWorkspace;

#[tokio::test]
async fn filters_completed_messages_at_the_command_output_boundary() {
    for (filter, expected) in [
        (vec![CliEventKind::Text], vec!["message:text"]),
        (vec![CliEventKind::Thought], vec!["message:thought"]),
        (vec![CliEventKind::Text, CliEventKind::Thought], vec!["message:text", "message:thought"]),
        (vec![CliEventKind::TurnEnded], vec!["turn:ended"]),
    ] {
        let output = HeadlessTest::new().events(filter).run().await;
        assert_eq!(output.exit, ExitCode::SUCCESS);
        assert!(output.stderr.is_empty(), "{}", output.stderr);
        let events = output.events();
        let kinds: Vec<_> = events
            .iter()
            .map(|event| match event {
                AgentEvent::Message(MessageEvent::Text { chunk, is_complete, .. }) => {
                    assert!(*is_complete);
                    assert_eq!(chunk, "hello world");
                    "message:text"
                }
                AgentEvent::Message(MessageEvent::Thought { chunk, is_complete, .. }) => {
                    assert!(*is_complete);
                    assert_eq!(chunk, "thinking hard");
                    "message:thought"
                }
                AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Completed }) => "turn:ended",
                event => panic!("unexpected event: {event:?}"),
            })
            .collect();
        assert_eq!(kinds, expected);
    }
}

#[tokio::test]
async fn default_output_omits_stream_fragments_but_keeps_tool_calls_and_results() {
    let output = HeadlessTest::new().with_tool_call().run().await;
    assert_eq!(output.exit, ExitCode::SUCCESS);
    let events = output.events();
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Tool(ToolEvent::Call { .. }))));
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Tool(ToolEvent::Result { .. }))));
    assert!(
        events.iter().any(|event| matches!(event, AgentEvent::Message(MessageEvent::Text { is_complete: true, .. })))
    );
    assert!(events.iter().any(|event| matches!(event, AgentEvent::Turn(TurnEvent::Ended { .. }))));
    assert!(!events.iter().any(|event| matches!(
        event,
        AgentEvent::Message(
            MessageEvent::Text { is_complete: false, .. } | MessageEvent::Thought { is_complete: false, .. }
        ) | AgentEvent::Tool(ToolEvent::InputStarted { .. } | ToolEvent::InputDelta { .. })
    )));
}

#[tokio::test]
async fn tool_filters_do_not_emit_other_events_or_change_success() {
    for filter in [vec![CliEventKind::ToolCall], vec![CliEventKind::ToolCall, CliEventKind::ToolResult]] {
        let expected = filter.len();
        let output = HeadlessTest::new().with_tool_call().events(filter).run().await;
        assert_eq!(output.exit, ExitCode::SUCCESS);
        let events = output.events();
        assert_eq!(events.len(), expected);
        assert!(matches!(&events[0], AgentEvent::Tool(ToolEvent::Call { .. })));
        if expected == 2 {
            assert!(matches!(&events[1], AgentEvent::Tool(ToolEvent::Result { .. })));
        }
    }
}

#[tokio::test]
async fn provider_failure_sets_exit_status_even_when_turn_end_is_filtered_out() {
    for filter in [vec![], vec![CliEventKind::ToolCall], vec![CliEventKind::TurnEnded]] {
        let includes_outcome = filter.is_empty() || filter.contains(&CliEventKind::TurnEnded);
        let output = HeadlessTest::new().failing().events(filter).run().await;
        assert_eq!(output.exit, ExitCode::FAILURE);
        assert_eq!(
            output.events().iter().any(|event| matches!(
                event,
                AgentEvent::Turn(TurnEvent::Ended { outcome: TurnOutcome::Failed { .. } })
            )),
            includes_outcome
        );
    }
}

#[tokio::test]
async fn text_output_routes_failures_to_stderr_and_prints_only_complete_text() {
    let output = HeadlessTest::new().format(OutputFormat::Text).events(vec![CliEventKind::Text]).run().await;
    assert_eq!(output.exit, ExitCode::SUCCESS);
    assert_eq!(output.stdout, "hello world\n");
    assert_eq!(output.stderr, "");

    let output =
        HeadlessTest::new().failing().format(OutputFormat::Text).events(vec![CliEventKind::TurnEnded]).run().await;
    assert_eq!(output.exit, ExitCode::FAILURE);
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("provider failed"));
}

struct HeadlessTest {
    workspace: TestWorkspace,
    config: RunConfig,
    provider: FakeLlmProvider,
}

struct CommandOutput {
    exit: ExitCode,
    stdout: String,
    stderr: String,
}

impl HeadlessTest {
    fn new() -> Self {
        let workspace = TestWorkspace::new();
        let cwd = workspace.root().to_path_buf();
        Self {
            workspace,
            config: RunConfig {
                prompt: "hello".into(),
                cwd: cwd.clone(),
                mcp_config: McpConfig::default(),
                spec: AgentSpec::default(),
                agent_catalog: AgentCatalog::new(cwd, vec![], None),
                system_prompt: None,
                output: OutputFormat::Json,
                verbose: false,
                events: vec![],
                oauth_credential_store: Arc::new(MemoryCredentialStore::new()),
                telemetry: None,
                trace_context: None,
            },
            provider: FakeLlmProvider::with_single_response(
                llm_response().reasoning(&["thinking", " hard"]).text(&["hello", " world"]).build(),
            ),
        }
    }

    fn events(mut self, events: Vec<CliEventKind>) -> Self {
        self.config.events = events;
        self
    }

    fn format(mut self, format: OutputFormat) -> Self {
        self.config.output = format;
        self
    }

    fn failing(mut self) -> Self {
        self.provider =
            FakeLlmProvider::with_single_response(vec![LlmResponse::Error { message: "provider failed".into() }]);
        self
    }

    fn with_tool_call(mut self) -> Self {
        self.config.mcp_config =
            serde_json::from_value(serde_json::json!({"servers": {"tasks": {"type": "in-memory"}}})).unwrap();
        self.provider = FakeLlmProvider::new(vec![
            llm_response().tool_call("tc1", "tasks__task_list", &["{", "}"]).build(),
            llm_response().text(&["hello", " world"]).build(),
        ]);
        self
    }

    async fn run(self) -> CommandOutput {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = HeadlessRunner::new(self.config)
            .with_provider(Arc::new(self.provider))
            .run(&mut stdout, &mut stderr)
            .await
            .unwrap();
        drop(self.workspace);
        CommandOutput { exit, stdout: String::from_utf8(stdout).unwrap(), stderr: String::from_utf8(stderr).unwrap() }
    }
}

impl CommandOutput {
    fn events(&self) -> Vec<AgentEvent> {
        self.stdout.lines().map(|line| serde_json::from_str(line).unwrap()).collect()
    }
}
