use acp_utils::ElicitationSchema;
use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
use acp_utils::notifications::{
    ContextUsage, ContextUsageParams, CreateElicitationRequestParams, ElicitationAction, ElicitationParams,
};
use acp_utils::testing::test_connection;
use agent_client_protocol::schema::{self as acp, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::buffer::{Buffer, Cell};
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::LocalSet;
use wisp::settings::WispSettings;
use wisp::workspace_status::WorkspaceStatus;
use wisp_next::app::{App, AppConfig};
use wisp_next::render::sync_terminal;

fn make_app() -> (App, UnboundedReceiver<PromptCommand>) {
    let (prompt_handle, command_rx) = AcpPromptHandle::recording();
    let app = App::new(AppConfig {
        session_id: SessionId::new("test-session"),
        agent_name: "aether".to_string(),
        workspace_status: WorkspaceStatus::new("~/code/demo", Some("main".to_string())),
        prompt_handle,
        settings: WispSettings::default(),
    });
    (app, command_rx)
}

fn make_terminal() -> Terminal<TestBackend> {
    Terminal::with_options(TestBackend::new(40, 15), TerminalOptions { viewport: Viewport::Inline(15) }).unwrap()
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn submit_prompt(app: &mut App, text: &str) {
    type_text(app, text);
    app.on_key(key(KeyCode::Enter));
}

fn session_update(update: acp::SessionUpdate) -> AcpEvent {
    AcpEvent::SessionUpdate { session_id: SessionId::new("test-session"), update: Box::new(update) }
}

fn text_chunk(text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    ))))
}

fn tool_call(id: &str, title: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title)))
}

fn tool_completed(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", Cell::symbol));
        }
        out.push('\n');
    }
    out
}

#[test]
fn submit_sends_prompt_and_clears_composer() {
    let (mut app, mut command_rx) = make_app();

    submit_prompt(&mut app, "hello agent");

    match command_rx.try_recv().unwrap() {
        PromptCommand::Prompt { session_id, text, content } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(text, "hello agent");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
    assert!(app.composer().is_empty());
}

#[test]
fn submit_is_ignored_while_prompt_in_flight() {
    let (mut app, mut command_rx) = make_app();
    submit_prompt(&mut app, "first");
    command_rx.try_recv().unwrap();

    submit_prompt(&mut app, "second");

    assert!(command_rx.try_recv().is_err());
    assert_eq!(app.composer().text(), "second");
}

#[test]
fn esc_cancels_only_while_busy() {
    let (mut app, mut command_rx) = make_app();

    app.on_key(key(KeyCode::Esc));
    assert!(command_rx.try_recv().is_err());

    submit_prompt(&mut app, "work");
    command_rx.try_recv().unwrap();
    app.on_key(key(KeyCode::Esc));

    assert!(matches!(command_rx.try_recv().unwrap(), PromptCommand::Cancel { .. }));
}

#[test]
fn double_ctrl_c_exits_and_first_press_clears_composer() {
    let (mut app, _command_rx) = make_app();
    type_text(&mut app, "draft");

    app.on_key(ctrl('c'));
    assert!(!app.exit_requested());
    assert!(app.composer().is_empty());
    assert!(app.exit_confirmation_active());

    app.on_key(ctrl('c'));
    assert!(app.exit_requested());
}

#[test]
fn ctrl_c_confirmation_disarms_after_window() {
    let (mut app, _command_rx) = make_app();

    app.on_key(ctrl('c'));
    app.on_tick(Instant::now() + Duration::from_secs(2));
    assert!(!app.exit_confirmation_active());

    app.on_key(ctrl('c'));
    assert!(!app.exit_requested());
}

#[test]
fn connection_closed_requests_exit() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(AcpEvent::ConnectionClosed);

    assert!(app.exit_requested());
}

#[test]
fn alt_enter_inserts_newline_instead_of_submitting() {
    let (mut app, mut command_rx) = make_app();
    type_text(&mut app, "line one");

    app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    type_text(&mut app, "line two");

    assert!(command_rx.try_recv().is_err());
    assert_eq!(app.composer().text(), "line one\nline two");
}

#[test]
fn context_usage_is_reported_as_percent() {
    let (mut app, _command_rx) = make_app();

    app.on_acp_event(AcpEvent::ContextUsage(ContextUsageParams {
        usage: ContextUsage { context_limit: Some(200_000), input_tokens: 50_000, ..ContextUsage::default() },
    }));

    assert_eq!(app.context_percent(), Some(25));
}

#[test]
fn user_message_lands_in_scrollback_immediately() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();

    submit_prompt(&mut app, "hello scrollback");
    sync_terminal(&mut terminal, &mut app).unwrap();

    assert!(buffer_text(terminal.backend().scrollback()).contains("hello scrollback"));
}

#[test]
fn streaming_text_stays_live_until_prompt_done_then_lands_once() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "hi");

    app.on_acp_event(text_chunk("streamed "));
    app.on_acp_event(text_chunk("answer"));
    sync_terminal(&mut terminal, &mut app).unwrap();

    assert!(buffer_text(terminal.backend().buffer()).contains("streamed answer"));
    assert!(!buffer_text(terminal.backend().scrollback()).contains("streamed answer"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal(&mut terminal, &mut app).unwrap();
    sync_terminal(&mut terminal, &mut app).unwrap();

    let scrollback = buffer_text(terminal.backend().scrollback());
    assert_eq!(scrollback.matches("streamed answer").count(), 1);
    assert!(!buffer_text(terminal.backend().buffer()).contains("streamed answer"));
}

#[test]
fn running_tool_holds_later_content_out_of_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    submit_prompt(&mut app, "run a tool");

    app.on_acp_event(tool_call("tool-1", "Reading main.rs"));
    app.on_acp_event(text_chunk("tool output summary"));
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(terminal.backend().buffer());
    assert!(viewport.contains("Reading main.rs"));
    assert!(!buffer_text(terminal.backend().scrollback()).contains("Reading main.rs"));

    app.on_acp_event(tool_completed("tool-1"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));
    sync_terminal(&mut terminal, &mut app).unwrap();

    let scrollback = buffer_text(terminal.backend().scrollback());
    assert!(scrollback.contains("Reading main.rs"));
    assert!(scrollback.contains("tool output summary"));
    assert!(app.pending_items().is_empty());
}

#[test]
fn cancelled_prompt_marks_running_tool_as_error() {
    let (mut app, _command_rx) = make_app();
    submit_prompt(&mut app, "run a tool");
    app.on_acp_event(tool_call("tool-1", "Slow tool"));

    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::Cancelled));

    let items = app.drain_finalized();
    let cancelled = items.iter().any(|item| {
        matches!(item, wisp_next::app::HistoryItem::Tool { status: wisp_next::tool_calls::ToolStatus::Error(cause), .. } if cause == "cancelled")
    });
    assert!(cancelled, "expected cancelled tool in {items:?}");
}

#[test]
fn composer_echo_and_status_line_render_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();

    type_text(&mut app, "typing");
    sync_terminal(&mut terminal, &mut app).unwrap();

    let viewport = buffer_text(terminal.backend().buffer());
    assert!(viewport.contains("> typing"));
    assert!(viewport.contains("~/code/demo · main"));
    assert!(viewport.contains("aether"));
}

#[test]
fn elicitation_request_is_auto_cancelled() {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, async {
        let (mut app, _command_rx) = make_app();
        let (cx, mut peer) = test_connection().await;
        let (responder, response_rx) = peer.fake_elicitation(&cx).await;

        app.on_acp_event(AcpEvent::ElicitationRequest {
            params: ElicitationParams {
                server_name: "test-server".to_string(),
                request: CreateElicitationRequestParams::FormElicitationParams {
                    meta: None,
                    message: String::new(),
                    requested_schema: ElicitationSchema::builder().build().unwrap(),
                },
            },
            responder,
        });

        let response = response_rx.await.unwrap();
        assert_eq!(response.action, ElicitationAction::Cancel);
        assert!(response.content.is_none());
    });
}
