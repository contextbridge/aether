use acp_utils::client::{AcpEvent, AcpPromptHandle, PromptCommand};
use acp_utils::notifications::SessionPreviewResponse;
use acp_utils::notifications::{AetherCapabilities, WorkspaceListResponse};
use acp_utils::notifications::{ElicitationParams, WorkspaceEntry};
use acp_utils::notifications::{ElicitationResponse, WorkspaceMoveResponse};
use agent_client_protocol::Responder;
use agent_client_protocol::schema::{self as acp, AuthMethod, SessionCapabilities, SessionConfigOption};
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedReceiver;
pub(super) use tui::KeyCode::{Backspace, Down, Enter, Esc, Up};
use tui::Renderer as FrameRenderer;
use tui::RendererCommand;
use tui::Theme;
use tui::display_width_text;
use tui::testing::TestTerminal;
use tui::{Component, Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseEvent, MouseEventKind};
use wisp::components::app::{App, AppInfo, EventOutcome};
use wisp::settings::DEFAULT_CONTENT_PADDING;
use wisp::workspace_status::WorkspaceStatus;

pub(super) const TEST_AGENT: &str = "test-agent";
pub(super) const TEST_WIDTH: u16 = 200;
pub(super) const TEST_WORKSPACE_DIR: &str = "~/code/foo";
pub(super) const TEST_GIT_REF: &str = "main";

/// Result type for tests and helpers so fallible operations propagate with `?` instead of panicking.
pub(super) type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

pub(super) fn test_workspace_status() -> WorkspaceStatus {
    WorkspaceStatus::new(TEST_WORKSPACE_DIR, Some(TEST_GIT_REF.to_string()))
}

pub(super) fn p(s: &str) -> String {
    format!("{}{s}", " ".repeat(DEFAULT_CONTENT_PADDING))
}

pub(super) fn expected_status_line(width: u16, agent_name: &str) -> String {
    let lines = expected_status_lines(width, agent_name);
    lines.into_iter().next().unwrap_or_default()
}

pub(super) fn expected_status_lines(width: u16, agent_name: &str) -> Vec<String> {
    let left = format!("{}{} · {}", " ".repeat(DEFAULT_CONTENT_PADDING), TEST_WORKSPACE_DIR, TEST_GIT_REF);
    let left_w = display_width_text(&left);
    let right_w = display_width_text(agent_name);
    let width = usize::from(width);

    if left_w + 1 + right_w <= width {
        let padding = width.saturating_sub(left_w + right_w);
        vec![truncate_to_display_width(&format!("{left}{}{agent_name}", " ".repeat(padding)), width)]
    } else {
        let row1 = truncate_to_display_width(&left, width);
        let pad = width.saturating_sub(right_w);
        let row2 = format!("{}{agent_name}", " ".repeat(pad));
        vec![row1, row2]
    }
}

/// Expected status-line rows, wrapping the right side onto a second line when
/// the left and right clusters do not fit together on one row.
pub(super) fn expected_status_line_rows(width: u16, agent_name: &str) -> Vec<String> {
    let left = format!("{}{} · {}", " ".repeat(DEFAULT_CONTENT_PADDING), TEST_WORKSPACE_DIR, TEST_GIT_REF);
    let w = usize::from(width);
    if display_width_text(&left) + display_width_text(agent_name) <= w {
        return vec![expected_status_line(width, agent_name)];
    }
    let pad = " ".repeat(DEFAULT_CONTENT_PADDING);
    vec![truncate_to_display_width(&left, w), truncate_to_display_width(&format!("{pad}{agent_name}"), w)]
}

fn truncate_to_display_width(text: &str, width: usize) -> String {
    let mut truncated = String::new();
    for ch in text.chars() {
        let next = format!("{truncated}{ch}");
        if display_width_text(&next) > width {
            break;
        }
        truncated = next;
    }
    truncated
}
/// Expected progress-indicator line for the first inactive→active transition.
pub(super) const PROGRESS_LINE: &str =
    "⠒ Tip: Hit Tab to adjust reasoning level (off → low → medium → high)  (esc to interrupt)";

pub(super) fn preview_session_capabilities() -> acp::SessionCapabilities {
    acp::SessionCapabilities::new()
        .meta(Some(AetherCapabilities { prompt_search: false, session_preview: true, workspace_move: false }.to_meta()))
}

pub(super) fn prompt_search_session_capabilities() -> acp::SessionCapabilities {
    acp::SessionCapabilities::new()
        .meta(Some(AetherCapabilities { prompt_search: true, session_preview: true, workspace_move: false }.to_meta()))
}

pub(super) fn workspace_session_capabilities() -> acp::SessionCapabilities {
    acp::SessionCapabilities::new()
        .meta(Some(AetherCapabilities { prompt_search: true, session_preview: true, workspace_move: true }.to_meta()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LoopAction {
    Continue,
    Exit,
}

pub(super) struct Renderer {
    app: App,
    frame: FrameRenderer<TestTerminal>,
    commands: UnboundedReceiver<PromptCommand>,
}

#[must_use]
pub(super) struct RendererTest {
    size: (u16, u16),
    agent_name: String,
    config_options: Vec<SessionConfigOption>,
    session_capabilities: SessionCapabilities,
    auth_methods: Vec<AuthMethod>,
}

impl Default for RendererTest {
    fn default() -> Self {
        Self::new()
    }
}

impl RendererTest {
    pub(super) fn new() -> Self {
        Self {
            size: (TEST_WIDTH, 40),
            agent_name: TEST_AGENT.to_string(),
            config_options: Vec::new(),
            session_capabilities: preview_session_capabilities(),
            auth_methods: Vec::new(),
        }
    }

    pub(super) fn size(mut self, size: (u16, u16)) -> Self {
        self.size = size;
        self
    }

    pub(super) fn agent_name(mut self, agent_name: impl Into<String>) -> Self {
        self.agent_name = agent_name.into();
        self
    }

    pub(super) fn config_options(mut self, config_options: &[acp::SessionConfigOption]) -> Self {
        self.config_options = config_options.to_vec();
        self
    }

    pub(super) fn session_capabilities(mut self, session_capabilities: acp::SessionCapabilities) -> Self {
        self.session_capabilities = session_capabilities;
        self
    }

    pub(super) fn auth_methods(mut self, auth_methods: Vec<acp::AuthMethod>) -> Self {
        self.auth_methods = auth_methods;
        self
    }

    pub(super) fn build(self) -> TestResult<Renderer> {
        let (prompt_handle, commands) = AcpPromptHandle::recording();
        let app = App::new(AppInfo {
            session_id: acp::SessionId::new("test"),
            agent_name: self.agent_name,
            prompt_capabilities: acp::PromptCapabilities::new(),
            session_capabilities: self.session_capabilities,
            config_options: self.config_options,
            auth_methods: self.auth_methods,
            working_dir: PathBuf::from("."),
            workspace_status: test_workspace_status(),
            prompt_handle,
            settings: wisp::settings::WispSettings::default()
                .with_default_status_line(wisp::settings::StatusLineSettings::defaults()),
        });
        let terminal = TestTerminal::new(self.size.0, self.size.1);
        let frame = FrameRenderer::new(terminal, Theme::default(), self.size);
        let mut renderer = Renderer { app, frame, commands };
        renderer.initial_render()?;
        Ok(renderer)
    }
}

impl Renderer {
    pub(super) fn needs_mouse_capture(&self) -> bool {
        self.app.needs_mouse_capture()
    }

    pub(super) fn writer(&self) -> &TestTerminal {
        self.frame.writer()
    }

    /// Prompt commands the app has dispatched, for tests that assert on dispatched commands.
    pub(super) fn commands(&mut self) -> &mut UnboundedReceiver<PromptCommand> {
        &mut self.commands
    }

    pub(super) fn test_writer_mut(&mut self) -> &mut TestTerminal {
        self.frame.test_writer_mut()
    }

    pub(super) fn render(&mut self) -> std::io::Result<()> {
        self.frame.render_frame(|ctx| self.app.render(ctx))
    }

    pub(super) fn initial_render(&mut self) -> std::io::Result<()> {
        self.render()
    }

    pub(super) async fn on_key_event(
        &mut self,
        key_event: tui::KeyEvent,
    ) -> Result<LoopAction, Box<dyn std::error::Error>> {
        self.handle_terminal_event(Event::Key(key_event)).await
    }

    pub(super) fn on_session_update(&mut self, update: acp::SessionUpdate) -> Result<(), Box<dyn std::error::Error>> {
        self.on_session_update_for(acp::SessionId::new("test"), update)
    }

    pub(super) fn on_session_update_for(
        &mut self,
        session_id: acp::SessionId,
        update: acp::SessionUpdate,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::SessionUpdate { session_id, update: Box::new(update) })?;
        Ok(())
    }

    pub(super) fn on_sessions_listed(
        &mut self,
        sessions: Vec<acp::SessionInfo>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::SessionsListed { sessions })?;
        Ok(())
    }

    pub(super) fn on_session_loaded(
        &mut self,
        session_id: acp::SessionId,
        config_options: Vec<acp::SessionConfigOption>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::SessionLoaded { session_id, config_options })?;
        Ok(())
    }

    pub(super) fn on_prompt_done(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn))?;
        Ok(())
    }

    pub(super) async fn on_tick(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_terminal_event(Event::Tick).await?;
        Ok(())
    }

    pub(super) async fn on_mouse_scroll_down(&mut self) -> Result<LoopAction, Box<dyn std::error::Error>> {
        let mouse = MouseEvent { kind: MouseEventKind::ScrollDown, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        self.handle_terminal_event(Event::Mouse(mouse)).await
    }

    pub(super) async fn on_mouse_scroll_up(&mut self) -> Result<LoopAction, Box<dyn std::error::Error>> {
        let mouse = MouseEvent { kind: MouseEventKind::ScrollUp, column: 0, row: 0, modifiers: KeyModifiers::NONE };
        self.handle_terminal_event(Event::Mouse(mouse)).await
    }

    pub(super) async fn on_mouse_down(
        &mut self,
        row: u16,
        column: u16,
    ) -> Result<LoopAction, Box<dyn std::error::Error>> {
        let mouse = MouseEvent {
            kind: MouseEventKind::Down(tui::MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };
        self.handle_terminal_event(Event::Mouse(mouse)).await
    }

    pub(super) async fn on_paste(&mut self, text: &str) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_terminal_event(Event::Paste(text.to_string())).await?;
        Ok(())
    }

    pub(super) async fn on_resize_event(&mut self, cols: u16, rows: u16) -> Result<(), Box<dyn std::error::Error>> {
        self.frame.on_resize((cols, rows));
        self.handle_terminal_event(Event::Resize((cols, rows).into())).await?;
        Ok(())
    }

    pub(super) fn on_context_usage(
        &mut self,
        params: acp_utils::notifications::ContextUsageParams,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::ContextUsage(params))?;
        Ok(())
    }

    pub(super) fn on_context_cleared(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::ContextCleared(acp_utils::notifications::ContextClearedParams::default()))?;
        Ok(())
    }

    pub(super) fn on_sub_agent_progress(
        &mut self,
        params: acp_utils::notifications::SubAgentProgressParams,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::SubAgentProgress(params))?;
        Ok(())
    }

    pub(super) fn on_auth_methods_updated(
        &mut self,
        params: acp_utils::notifications::AuthMethodsUpdatedParams,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::AuthMethodsUpdated(params))?;
        Ok(())
    }

    pub(super) fn on_elicitation_request(
        &mut self,
        params: ElicitationParams,
        responder: Responder<ElicitationResponse>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::ElicitationRequest { params, responder })?;
        Ok(())
    }

    pub(super) fn on_prompt_search_results(
        &mut self,
        response: acp_utils::notifications::PromptSearchResponse,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::PromptSearchResults(response))?;
        Ok(())
    }

    pub(super) fn on_prompt_search_failed(
        &mut self,
        query: &str,
        error: impl Into<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::PromptSearchFailed { query: query.to_string(), error: error.into() })?;
        Ok(())
    }

    pub(super) fn on_session_preview_loaded(
        &mut self,
        preview: SessionPreviewResponse,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::SessionPreviewLoaded(preview))?;
        Ok(())
    }

    pub(super) fn on_session_preview_failed(
        &mut self,
        session_id: &str,
        error: impl Into<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::SessionPreviewFailed {
            session_id: session_id.to_string(),
            error: error.into(),
        })?;
        Ok(())
    }

    pub(super) fn on_mcp_notification(
        &mut self,
        notification: acp_utils::notifications::McpNotification,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::McpNotification(notification))?;
        Ok(())
    }

    pub(super) fn on_workspaces_listed(
        &mut self,
        workspaces: Vec<WorkspaceEntry>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::WorkspacesListed(WorkspaceListResponse { workspaces }))?;
        Ok(())
    }

    pub(super) fn on_workspace_moved(&mut self, new_cwd: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::WorkspaceMoved(WorkspaceMoveResponse { new_cwd }))?;
        Ok(())
    }

    pub(super) fn on_workspace_move_failed(
        &mut self,
        error: impl Into<String>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::WorkspaceMoveFailed { error: error.into() })?;
        Ok(())
    }

    pub(super) fn on_connection_closed(&mut self) -> Result<LoopAction, Box<dyn std::error::Error>> {
        self.handle_acp_event(AcpEvent::ConnectionClosed)
    }

    async fn handle_terminal_event(&mut self, event: Event) -> Result<LoopAction, Box<dyn std::error::Error>> {
        let commands = self.app.on_event(&event).await.unwrap_or_default();
        self.drain_and_render(commands)
    }

    fn handle_acp_event(&mut self, event: AcpEvent) -> Result<LoopAction, Box<dyn std::error::Error>> {
        let outcome = self.app.on_acp_event(event);
        if self.app.exit_requested() {
            return Ok(LoopAction::Exit);
        }

        if let EventOutcome::Render { commands } = outcome {
            self.frame.apply_commands(commands)?;
            self.render()?;
        }
        Ok(LoopAction::Continue)
    }

    fn drain_and_render(&mut self, commands: Vec<RendererCommand>) -> Result<LoopAction, Box<dyn std::error::Error>> {
        self.frame.apply_commands(commands)?;

        if self.app.exit_requested() {
            return Ok(LoopAction::Exit);
        }

        self.render()?;
        Ok(LoopAction::Continue)
    }
}

/// Test events that can be fed to the renderer.
pub(super) enum TestEvent {
    Update(Box<acp::SessionUpdate>),
    PromptDone,
}

/// Build the expected prompt lines for a given terminal width.
/// Returns `[top_rule, input_line, bottom_rule, status_line]`.
pub(super) fn expected_prompt(width: u16, input: &str, agent_name: &str) -> Vec<String> {
    let w = width as usize;
    let top = "─".repeat(w);
    let middle = format!("> {input}").trim_end().to_string();
    let bottom = "─".repeat(w);
    let status = expected_status_line(width, agent_name);
    vec![top, middle, bottom, status]
}

/// Build expected lines: scrollback lines + bordered prompt.
pub(super) fn expected_with_prompt(scrollback: &[&str], width: u16, input: &str, agent_name: &str) -> Vec<String> {
    let mut lines: Vec<String> = scrollback.iter().map(ToString::to_string).collect();
    lines.extend(expected_prompt(width, input, agent_name));
    lines
}

pub(super) fn has_file_picker(terminal: &TestTerminal) -> bool {
    let lines = terminal.get_lines();
    lines.iter().any(|l| {
        l.contains("(no matches found)")
            || (l.starts_with("  ") && (l.contains('/') || l.contains('.')) && !l.contains(TEST_AGENT))
    })
}

pub(super) fn has_command_picker(terminal: &TestTerminal) -> bool {
    let lines = terminal.get_lines();
    lines.iter().any(|l| l.contains("Open settings"))
}

pub(super) fn has_settings_menu(terminal: &TestTerminal) -> bool {
    let lines = terminal.get_lines();
    lines.iter().any(|l| l.contains("Configuration"))
}

pub(super) fn has_settings_picker(terminal: &TestTerminal) -> bool {
    let lines = terminal.get_lines();
    lines.iter().any(|l| l.contains("search:"))
}

#[allow(dead_code)]
pub(super) fn settings_menu_selected_label(terminal: &TestTerminal) -> Option<String> {
    let theme = Theme::default();
    let bg_color = theme.highlight_bg();
    let fg_color = theme.highlight_fg();
    let lines = terminal.get_lines();
    let width = terminal.get_lines().first().map_or(80, |l| l.len().max(80));
    // Scan each row for any cell with highlight_bg to find the selected row.
    // The menu is inside a bordered overlay so we need to check multiple columns.
    for (row, line) in lines.iter().enumerate() {
        let has_highlight = (0..width).any(|col| {
            let style = terminal.get_style_at(row, col);
            style.bg == Some(bg_color) || style.fg == Some(fg_color)
        });
        if has_highlight {
            let label = line.trim().to_string();
            if !label.is_empty() {
                return Some(label);
            }
        }
    }
    None
}

pub(super) fn command_picker_visible_names(terminal: &TestTerminal) -> Vec<String> {
    let lines = terminal.get_lines();
    let mut names = Vec::new();
    for line in &lines {
        // Match lines like "  /name  description"
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('/')
            && let Some(name) = rest.split_whitespace().next()
        {
            names.push(name.to_string());
        }
    }
    names
}

pub(super) fn render_with_size(events: Vec<TestEvent>, size: (u16, u16)) -> TestResult<Renderer> {
    let mut renderer = RendererTest::new().size(size).build()?;

    for event in events {
        match event {
            TestEvent::Update(update) => renderer.on_session_update(*update)?,
            TestEvent::PromptDone => renderer.on_prompt_done()?,
        }
    }

    Ok(renderer)
}

pub(super) fn render(events: Vec<TestEvent>) -> TestResult<Renderer> {
    render_with_size(events, (TEST_WIDTH, 40))
}

pub(super) fn text_chunk(text: &str) -> TestEvent {
    TestEvent::Update(Box::new(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    )))))
}

pub(super) fn thought_chunk(text: &str) -> TestEvent {
    TestEvent::Update(Box::new(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(acp::ContentBlock::Text(
        acp::TextContent::new(text),
    )))))
}

pub(super) fn prompt_done() -> TestEvent {
    TestEvent::PromptDone
}

pub(super) fn tool_call(name: &str, args: &str) -> TestEvent {
    tool_call_with_id(name, &format!("call_{name}"), args)
}

pub(super) fn tool_call_with_id(name: &str, id: &str, args: &str) -> TestEvent {
    let mut tc = acp::ToolCall::new(id.to_string(), name);
    if !args.is_empty() {
        let value: serde_json::Value =
            serde_json::from_str(args).unwrap_or_else(|_| serde_json::Value::String(args.to_string()));
        tc = tc.raw_input(value);
    }
    TestEvent::Update(Box::new(acp::SessionUpdate::ToolCall(tc)))
}

pub(super) fn tool_complete(id: &str) -> TestEvent {
    TestEvent::Update(Box::new(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    ))))
}

pub(super) fn tool_complete_with_display_meta(id: &str, display_meta: &serde_json::Value) -> TestEvent {
    let title = display_meta["title"].as_str().unwrap_or("");
    let value = display_meta["value"].as_str().unwrap_or("");

    let mut meta_map = serde_json::Map::new();
    if !value.is_empty() {
        meta_map.insert("display_value".into(), value.into());
    }

    let mut update = acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().title(title).status(acp::ToolCallStatus::Completed),
    );
    if !meta_map.is_empty() {
        update = update.meta(meta_map);
    }
    TestEvent::Update(Box::new(acp::SessionUpdate::ToolCallUpdate(update)))
}

pub(super) fn tool_update_with_args(id: &str, args: &str) -> TestEvent {
    let value: serde_json::Value = serde_json::from_str(args).unwrap();
    TestEvent::Update(Box::new(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().raw_input(value),
    ))))
}

pub(super) async fn type_string(renderer: &mut Renderer, text: &str) -> TestResult {
    for ch in text.chars() {
        let key_event = KeyEvent {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: KeyEventState::empty(),
        };
        renderer.on_key_event(key_event).await?;
    }
    Ok(())
}

pub(super) async fn press(renderer: &mut Renderer, code: KeyCode) -> TestResult {
    press_with_modifiers(renderer, code, KeyModifiers::empty()).await
}

pub(super) async fn press_with_modifiers(
    renderer: &mut Renderer,
    code: KeyCode,
    modifiers: KeyModifiers,
) -> TestResult {
    renderer
        .on_key_event(KeyEvent { code, modifiers, kind: KeyEventKind::Press, state: KeyEventState::empty() })
        .await?;
    Ok(())
}

pub(super) fn make_settings_options() -> Vec<acp::SessionConfigOption> {
    vec![
        acp::SessionConfigOption::select(
            "model".to_string(),
            "Model".to_string(),
            "openrouter:openai/gpt-4o".to_string(),
            vec![
                acp::SessionConfigSelectOption::new(
                    "openrouter:openai/gpt-4o".to_string(),
                    "OpenRouter / GPT-4o".to_string(),
                ),
                acp::SessionConfigSelectOption::new(
                    "anthropic:claude-sonnet-4-5".to_string(),
                    "Anthropic / Claude Sonnet 4.5".to_string(),
                ),
            ],
        )
        .category(acp::SessionConfigOptionCategory::Model),
    ]
}

/// Create a renderer with settings options and open the settings menu.
pub(super) async fn open_settings(
    config_options: &[acp::SessionConfigOption],
    size: (u16, u16),
) -> TestResult<Renderer> {
    let mut renderer = RendererTest::new().size(size).config_options(config_options).build()?;
    type_string(&mut renderer, "/settings").await?;
    press(&mut renderer, Enter).await?;
    Ok(renderer)
}

pub(super) async fn open_resume_picker(renderer: &mut Renderer) -> TestResult {
    type_string(renderer, "/resume").await?;
    press(renderer, Enter).await
}

pub(super) async fn load_session_via_picker(
    renderer: &mut Renderer,
    session_id: acp::SessionId,
    cwd: PathBuf,
) -> TestResult {
    open_resume_picker(renderer).await?;
    let info = acp::SessionInfo::new(session_id, cwd).title("Resumable session".to_string());
    renderer.on_sessions_listed(vec![info])?;
    press(renderer, Enter).await
}

/// Assert that any terminal line contains the given text, panicking with a buffer dump.
pub(super) fn assert_buffer_contains(terminal: &TestTerminal, text: &str) {
    let lines = terminal.get_lines();
    assert!(
        lines.iter().any(|l| l.contains(text)),
        "Expected to find '{text}' in buffer.\nBuffer:\n{}",
        lines.join("\n")
    );
}

/// Assert that no terminal line contains the given text.
pub(super) fn assert_buffer_not_contains(terminal: &TestTerminal, text: &str) {
    let lines = terminal.get_lines();
    assert!(
        !lines.iter().any(|l| l.contains(text)),
        "Expected NOT to find '{text}' in buffer.\nBuffer:\n{}",
        lines.join("\n")
    );
}
