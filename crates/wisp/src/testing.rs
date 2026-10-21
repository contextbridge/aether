//! Public testing harness for driving the application model without side effects.
//!
//! Prefer [`TestUi`] over reaching for [`App`], [`Renderer`], or a raw
//! `Terminal<TestBackend>`: it owns all of them and routes input, ACP events,
//! task settling, and drawing through the same seams the event loop uses.

use crate::app::message::Message;
use crate::app::{App, AppConfig};
use crate::command::{Command, CommandResult, HostCommand};
use crate::git_review::{ClientState, ConnectionState, DiffDocument, DiffScope, DiffSnapshot, FileDiff};
pub use crate::renderer::RenderStats;
use crate::renderer::{DrawContext, Renderer};
use crate::screens::git_diff::GitDiffScreen;
use crate::session::platform::BrowserOpener;
use crate::session::terminal::inline_viewport_height;
use crate::settings::UiSettings;
use crate::surfaces::composer::ComposerLayout;
use crate::theme::Theme;
use crate::view::generation::Generation;
use crate::view::syntax::SyntaxHighlighter;
use acp_utils::client::AcpEvent;
use acp_utils::content::display_content_blocks;
use acp_utils::notifications::{ContextClearedParams, SubAgentEvent, SubAgentProgressParams};
use acp_utils::testing::{idle_notification, running_notification};
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2::{self as acp, SessionId, SessionUpdate, ToolCallUpdate};
use clankerdiff_core::git_patch_from_texts;
use client_core::command::{AgentCommand, PromptRejection};
use client_core::request::RequestId;
use client_core::session::Session;
use client_core::testing::{FakeExecutor, FakeGit, SessionCoreConfigBuilder};
use client_core::workspace_status::WorkspaceStatus;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde_json::json;
use std::borrow::Borrow;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;
use unicode_width::UnicodeWidthStr;

#[derive(Default)]
struct FakeAgent {
    running: bool,
    queued: VecDeque<SentPrompt>,
    inserted: usize,
    rejection: Option<PromptRejection>,
}

struct SentPrompt {
    request_id: RequestId,
    content: Vec<acp::ContentBlock>,
}

impl FakeAgent {
    fn observe(&mut self, message: &Message, session_id: &SessionId) {
        let Message::Agent(event) = message else { return };
        match event.as_ref() {
            AcpEvent::SessionUpdate(notification) if notification.session_id == *session_id => {
                match notification.update {
                    acp::SessionUpdate::StateUpdate(acp::StateUpdate::Running(_)) => self.running = true,
                    acp::SessionUpdate::StateUpdate(acp::StateUpdate::Idle(_)) => self.running = false,
                    _ => {}
                }
            }
            AcpEvent::ConnectionClosed => {
                self.running = false;
                self.queued.clear();
            }
            _ => {}
        }
    }

    fn receive(&mut self, commands: &[Command]) {
        for command in commands {
            if let Command::Agent(AgentCommand::Prompt { request_id, text, content, .. }) = command {
                let mut blocks = vec![acp::ContentBlock::from(text.clone())];
                blocks.extend(content.iter().flatten().cloned());
                self.queued.push_back(SentPrompt { request_id: *request_id, content: blocks });
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendEvent {
    ShowCursor,
    Scroll,
}

/// frame pushed at the terminal. Because ratatui diffs buffers before flushing,
/// `cells_drawn` counts only cells that actually changed — a frame that
/// repaints a settled screen costs nothing.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BackendStats {
    pub draws: u64,
    pub cells_drawn: u64,
    pub scrolls: u64,
}

#[derive(Debug)]
pub struct InstrumentedBackend {
    inner: TestBackend,
    events: Vec<BackendEvent>,
    stats: BackendStats,
}

impl InstrumentedBackend {
    pub fn new(width: u16, height: u16) -> Self {
        Self { inner: TestBackend::new(width, height), events: Vec::new(), stats: BackendStats::default() }
    }

    pub fn take_events(&mut self) -> Vec<BackendEvent> {
        std::mem::take(&mut self.events)
    }

    pub fn take_stats(&mut self) -> BackendStats {
        std::mem::take(&mut self.stats)
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.inner.resize(width, height);
    }

    pub fn buffer(&self) -> &Buffer {
        self.inner.buffer()
    }

    pub fn scrollback(&self) -> &Buffer {
        self.inner.scrollback()
    }
}

impl Borrow<TestBackend> for InstrumentedBackend {
    fn borrow(&self) -> &TestBackend {
        &self.inner
    }
}

impl Backend for InstrumentedBackend {
    type Error = std::convert::Infallible;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.stats.draws += 1;
        let mut cells = 0u64;
        let drawn = self.inner.draw(content.inspect(|_| cells += 1));
        self.stats.cells_drawn += cells;
        drawn
    }

    fn append_lines(&mut self, lines: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(lines)
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::ShowCursor);
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::Scroll);
        self.stats.scrolls += 1;
        self.inner.scroll_region_up(region, lines)
    }

    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, lines: u16) -> Result<(), Self::Error> {
        self.events.push(BackendEvent::Scroll);
        self.stats.scrolls += 1;
        self.inner.scroll_region_down(region, lines)
    }
}

/// A whole UI scenario: the app, the renderer that owns its scrollback, the
/// terminal it draws into, and the receiver for the commands the app sends.
///
/// Construct through [`TestUi::new`], [`TestUi::with_dimensions`], or
/// [`TestUiBuilder`]. Input, ACP events, ticks, and task results are routed the
/// same way the event loop routes them, and drawing happens against the same
/// renderer instance every frame so committed scrollback survives.
pub struct TestUi<B: Backend = TestBackend> {
    app: App,
    renderer: Renderer,
    terminal: Terminal<B>,
    executor: FakeExecutor<HostCommand>,
    agent: FakeAgent,
    opened_urls: Arc<Mutex<Vec<String>>>,
}

impl<B: Backend> TestUi<B>
where
    B::Error: std::fmt::Debug,
{
    /// Builds a UI around an arbitrary backend (e.g. a recording backend that
    /// asserts on frame-level terminal commands). The app is the default
    /// scenario; configure anything else through [`TestUiBuilder`].
    pub fn with_backend(backend: B) -> Self {
        let builder = TestUiBuilder::new();
        let app = App::new(builder.app_config());
        builder.finish(app, backend)
    }

    /// The app under test, for the read-only queries assertions are built from.
    /// Drive it through this harness rather than mutating it directly, so the
    /// commands it emits still reach the fake runtime.
    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn deliver(&mut self, message: Message) {
        self.agent.observe(&message, self.app.session_id());
        let commands = self.app.update(message);
        self.agent.receive(&commands);
        self.executor.record(commands);
        if !self.agent.running {
            self.insert_queued_prompts();
        }
    }

    pub fn insert_queued_prompts(&mut self) {
        while let Some(prompt) = self.agent.queued.pop_front() {
            if let Some(rejection) = self.agent.rejection.clone() {
                self.deliver_result(CommandResult::Prompt { request_id: prompt.request_id, result: Err(rejection) });
                continue;
            }
            if !self.agent.running {
                self.acp_event(running_notification(self.app.session_id().clone()).into());
            }
            self.agent.inserted += 1;
            let message = acp::UserMessage::new(format!("user-message-{}", self.agent.inserted))
                .content(display_content_blocks(&prompt.content));
            self.acp_event(session_update(acp::SessionUpdate::UserMessage(message)));
            self.deliver_result(CommandResult::Prompt { request_id: prompt.request_id, result: Ok(()) });
        }
    }

    pub fn reject_queued_prompts(&mut self, rejection: &PromptRejection) {
        while let Some(prompt) = self.agent.queued.pop_front() {
            self.deliver_result(CommandResult::Prompt {
                request_id: prompt.request_id,
                result: Err(rejection.clone()),
            });
        }
    }

    pub fn reject_prompts(&mut self, rejection: PromptRejection) {
        self.agent.rejection = Some(rejection);
    }

    pub fn deliver_result(&mut self, result: CommandResult) {
        self.deliver(Message::CommandFinished(Box::new(result)));
    }

    /// Commands emitted by the public application boundary, in order.
    pub fn executor(&self) -> &FakeExecutor<HostCommand> {
        &self.executor
    }

    pub fn executor_mut(&mut self) -> &mut FakeExecutor<HostCommand> {
        &mut self.executor
    }

    pub fn take_commands(&mut self) -> Vec<Command> {
        self.executor.take_commands()
    }

    pub fn next_command(&mut self) -> Option<Command> {
        self.executor.next_command()
    }

    pub fn next_agent_command(&mut self) -> Option<AgentCommand> {
        while let Some(command) = self.next_command() {
            if let Command::Agent(command) = command {
                return Some(command);
            }
        }
        None
    }

    pub fn take_command_where<T>(&mut self, extract: impl FnMut(Command) -> Option<T>) -> Option<T> {
        self.take_commands().into_iter().find_map(extract)
    }

    pub fn count_commands(&mut self, predicate: impl Fn(&Command) -> bool) -> usize {
        self.take_commands().iter().filter(|command| predicate(command)).count()
    }

    pub fn backend(&self) -> &B {
        self.terminal.backend()
    }

    pub fn backend_mut(&mut self) -> &mut B {
        self.terminal.backend_mut()
    }

    pub fn viewport_area(&mut self) -> ratatui::layout::Rect {
        self.terminal.get_frame().area()
    }

    pub fn viewport_height(&mut self) -> u16 {
        self.viewport_area().height
    }

    /// Measures the composer at `width` with the active theme.
    pub fn composer_layout(&mut self, width: u16) -> ComposerLayout {
        let theme = self.app.theme().clone();
        let composer = self.app.composer_mut();
        composer.on_resize(width);
        composer.layout(width, &theme)
    }

    /// Draws one frame, like the event loop does after every input batch.
    pub fn draw(&mut self) {
        self.try_draw().unwrap();
    }

    pub fn try_draw(&mut self) -> Result<(), crate::error::RenderError<B::Error>> {
        self.renderer.draw(&mut self.terminal, &mut self.app)
    }

    pub fn render_stats(&mut self) -> RenderStats {
        self.renderer.take_stats()
    }

    /// URLs elicitation prompts asked to open, in order. The harness records
    /// them instead of spawning a browser, so no test touches the host.
    pub fn opened_urls(&self) -> Vec<String> {
        self.opened_urls.lock().unwrap().clone()
    }

    /// Seeds a deterministic long conversation: `turns` completed turns of
    /// user messages, thoughts, markdown prose with fenced code, bash and edit
    /// tool calls with diffs, and a sub-agent tree every eighth turn — drawing
    /// after each turn so scrollback commits exactly like a live session.
    pub fn seed_long_history(&mut self, turns: usize) {
        for turn in 0..turns {
            let prompt = format!("Turn {turn}: reconcile the writer path in module_{turn} and add a regression test.");
            self.submit(&prompt);
            self.acp_event(session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
                acp::ContentBlock::Text(acp::TextContent::new(format!(
                    "Reading module_{turn} to find the torn-update window before touching any call site."
                ))),
                format!("seed-thought-{turn}"),
            ))));
            self.acp_event(text_chunk_with_id(&format!("seed-response-{turn}"), SEED_PROSE));
            self.acp_event(text_chunk_with_id(&format!("seed-response-{turn}"), SEED_CODE_BLOCK));
            let bash = format!("seed-bash-{turn}");
            self.acp_event(seed_bash_tool(&bash));
            self.acp_event(tool_completed(&bash));
            let edit = format!("seed-edit-{turn}");
            self.acp_event(seed_edit_tool(&edit, turn));
            self.acp_event(seed_tool_diff(&edit, turn));
            if turn % 8 == 0 {
                self.seed_sub_agent_tree(turn);
            }
            self.acp_event(text_chunk_with_id(&format!("seed-closing-{turn}"), SEED_CLOSING));
            self.complete_prompt(acp::StopReason::EndTurn);
            self.draw();
        }
    }

    /// Streams one assistant message of `total_bytes` in `chunk_bytes` chunks,
    /// drawing after every chunk the way the event loop draws after every
    /// wakeup. The message stays one open item while it streams.
    pub fn stream_message(&mut self, content: StreamContent, total_bytes: usize, chunk_bytes: usize) {
        let thought = matches!(content, StreamContent::Thought);
        let message = match content {
            StreamContent::Prose => prose_message(total_bytes),
            StreamContent::CodeBlock => code_block_message(total_bytes),
            StreamContent::Thought => thought_message(total_bytes),
        };
        for chunk in chunk_message(&message, chunk_bytes.max(1)) {
            if thought {
                self.acp_event(thought_chunk(&chunk));
            } else {
                self.acp_event(text_chunk(&chunk));
            }
            self.draw();
        }
    }

    /// Finishes the in-flight turn and advances synthetic time past every
    /// grace period, leaving a session that owes the event loop nothing.
    pub fn settle(&mut self) {
        self.complete_prompt(acp::StopReason::EndTurn);
        let mut now = Instant::now();
        // The plan tracker's grace period (3s) is the longest deadline a
        // settled session can still be waiting on.
        for _ in 0..12 {
            self.tick(now);
            now += Duration::from_millis(500);
            if !self.app().wants_tick() {
                break;
            }
        }
        assert!(!self.app().wants_tick(), "a settled session must stop driving the tick loop");
        self.draw();
    }

    /// Routes a terminal event (key, paste, mouse, resize) the way the event
    /// loop does.
    pub fn terminal_event(&mut self, event: Event) {
        self.deliver(Message::Terminal(event));
    }

    pub fn key(&mut self, key: KeyEvent) {
        self.deliver(Message::Terminal(Event::Key(key)));
    }

    pub fn type_text(&mut self, text: &str) {
        for character in text.chars() {
            self.key(KeyEvent::new(KeyCode::Char(character), KeyModifiers::NONE));
        }
    }

    /// Types `text` and submits it with Enter.
    pub fn submit(&mut self, text: &str) {
        self.type_text(text);
        self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    pub fn turn(&mut self, prompt: &str, reply: &str) {
        self.submit(prompt);
        self.acp_event(text_chunk(reply));
        self.complete_prompt(acp::StopReason::EndTurn);
    }

    pub fn paste(&mut self, text: &str) {
        self.deliver(Message::Terminal(Event::Paste(text.to_string())));
    }

    pub fn acp_event(&mut self, event: AcpEvent) {
        self.deliver(Message::Agent(Box::new(event)));
    }

    pub fn run_command(&mut self, command: &str) {
        self.type_text(command);
        self.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }

    pub fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) {
        self.run_command("/resume");
        let listed = self.drain_agent_commands_until(|command| matches!(command, AgentCommand::ListSessions));
        assert!(listed, "/resume should ask the agent to list sessions");
        self.deliver_result(CommandResult::SessionsListed(Ok(acp::ListSessionsResponse::new(sessions))));
    }

    pub fn begin_resume(&mut self, session_id: &str, cwd: &str) {
        self.open_session_picker(vec![acp::SessionInfo::new(session_id.to_string(), cwd)]);
        self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let resumed = self.drain_agent_commands_until(|command| {
            matches!(command, AgentCommand::ResumeSession { session_id: id, cwd: dir }
                if id.0.as_ref() == session_id && dir == Path::new(cwd))
        });
        assert!(resumed, "picking {session_id} should ask the agent to resume it in {cwd}");
    }

    /// The agent reports that its turn is over.
    pub fn complete_prompt(&mut self, stop_reason: acp::StopReason) {
        self.acp_event(idle_notification(self.app.session_id().clone(), Some(stop_reason)).into());
    }

    pub fn tick(&mut self, now: Instant) {
        self.deliver(Message::Tick(now));
    }

    /// Completes the commands configured on the fake executor without touching
    /// the real filesystem, Git repository, terminal, or ACP connection.
    pub fn settle_tasks(&mut self) {
        // Delivering records into `self.executor`, so the settling executor is
        // moved out and handed back each result's commands instead.
        let mut executor = std::mem::take(&mut self.executor);
        executor.settle(|result| {
            self.deliver_result(result);
            self.executor.take_commands()
        });
        self.executor = executor;
    }

    fn seed_sub_agent_tree(&mut self, turn: usize) {
        let parent = format!("seed-spawn-{turn}");
        self.acp_event(seed_spawn_tool(&parent));
        self.acp_event(tool_completed(&parent));
        for agent in ["explorer", "fixer"] {
            let task = format!("{parent}-{agent}");
            let grep = format!("{task}-grep");
            self.acp_event(sub_agent_tool_update(
                &parent,
                &task,
                agent,
                ToolCallUpdate::new(grep.clone())
                    .title("Grep")
                    .name("coding__grep")
                    .raw_input(json!({ "pattern": "torn update" })),
            ));
            self.acp_event(sub_agent_tool_update(
                &parent,
                &task,
                agent,
                ToolCallUpdate::new(grep).status(acp::ToolCallStatus::Completed),
            ));
            self.acp_event(sub_agent_progress(&parent, &task, agent, SubAgentEvent::Done));
        }
    }

    fn drain_agent_commands_until(&mut self, predicate: impl Fn(&AgentCommand) -> bool) -> bool {
        std::iter::from_fn(|| self.next_agent_command()).any(|command| predicate(&command))
    }
}

impl TestUi<TestBackend> {
    /// The default scenario: 40x15 terminal, plain app, recording prompt handle.
    pub fn new() -> Self {
        Self::with_dimensions(40, 15)
    }

    pub fn with_dimensions(width: u16, height: u16) -> Self {
        TestUiBuilder::new().dimensions(width, height).build()
    }

    /// Resizes the backing terminal. The next [`Self::draw`] re-measures the
    /// inline viewport the way `Renderer::draw` does in the event loop.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.terminal.backend_mut().resize(width, height);
    }
}

impl Default for TestUi<TestBackend> {
    fn default() -> Self {
        Self::new()
    }
}

impl<B> TestUi<B>
where
    B: Backend + Borrow<TestBackend>,
    B::Error: std::fmt::Debug,
{
    /// What the inline viewport currently shows: the composer, status line,
    /// and the live tail of the conversation. Draws a frame first, so asserting
    /// right after an input never reads a stale screen.
    pub fn viewport(&mut self) -> Buffer {
        self.draw();
        viewport_buffer(&mut self.terminal)
    }

    /// The terminal's own native scrollback containing committed conversation
    /// rows, after drawing a frame.
    pub fn history(&mut self) -> Buffer {
        self.draw();
        history_buffer(&mut self.terminal)
    }

    /// [`Self::history`] stacked on [`Self::viewport`]: everything the
    /// conversation has shown, oldest at the top, after drawing a frame.
    pub fn conversation(&mut self) -> Buffer {
        self.draw();
        conversation_buffer(&mut self.terminal)
    }

    pub fn viewport_text(&mut self) -> String {
        buffer_text(&self.viewport())
    }

    pub fn history_text(&mut self) -> String {
        buffer_text(&self.history())
    }

    pub fn conversation_text(&mut self) -> String {
        buffer_text(&self.conversation())
    }

    /// Row (within the viewport buffer) of the first line containing `needle`.
    pub fn viewport_row(&mut self, needle: &str) -> Option<u16> {
        row_containing(&self.viewport(), needle)
    }

    pub fn click_text(&mut self, needle: &str) {
        let viewport = self.viewport();
        let rows: Vec<u16> = (viewport.area.top()..viewport.area.bottom())
            .filter(|&row| row_text(&viewport, row).contains(needle))
            .collect();
        let [row] = rows[..] else {
            panic!("expected one row showing {needle:?}, found rows {rows:?}:\n{}", buffer_text(&viewport));
        };
        let text = row_text(&viewport, row);
        let column = u16::try_from(text[..text.find(needle).expect("the row shows needle")].width()).unwrap();
        let kind = MouseEventKind::Down(MouseButton::Left);
        self.terminal_event(Event::Mouse(MouseEvent { kind, column, row, modifiers: KeyModifiers::NONE }));
    }

    pub fn assert_viewport_contains(&mut self, needle: &str) {
        assert_text_contains("viewport", &self.viewport_text(), needle, true);
    }

    pub fn assert_viewport_not_contains(&mut self, needle: &str) {
        assert_text_contains("viewport", &self.viewport_text(), needle, false);
    }

    pub fn assert_history_contains(&mut self, needle: &str) {
        assert_text_contains("history", &self.history_text(), needle, true);
    }

    pub fn assert_history_not_contains(&mut self, needle: &str) {
        assert_text_contains("history", &self.history_text(), needle, false);
    }

    pub fn assert_conversation_contains(&mut self, needle: &str) {
        assert_text_contains("conversation", &self.conversation_text(), needle, true);
    }

    pub fn assert_conversation_not_contains(&mut self, needle: &str) {
        assert_text_contains("conversation", &self.conversation_text(), needle, false);
    }

    /// Asserts the viewport's visible text matches `expected` row-by-row.
    pub fn assert_viewport<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.viewport(), expected);
    }

    /// Asserts the committed history's visible text matches `expected` row-by-row.
    pub fn assert_history<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.history(), expected);
    }

    /// Asserts the stitched conversation's visible text matches `expected` row-by-row.
    pub fn assert_conversation<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.conversation(), expected);
    }
}

/// Builds a [`TestUi`]: the terminal dimensions plus every app-scenario option
/// a test cares about. Defaults match a plain `make_app()`-style scenario.
pub struct TestUiBuilder {
    width: u16,
    height: u16,
    session: SessionCoreConfigBuilder,
    settings: UiSettings,
    git: FakeGit,
    opened_urls: Arc<Mutex<Vec<String>>>,
}

impl Default for TestUiBuilder {
    fn default() -> Self {
        Self {
            width: 40,
            height: 15,
            session: SessionCoreConfigBuilder::new().client_name(crate::client_info().name),
            settings: UiSettings::default(),
            git: FakeGit::default(),
            opened_urls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl TestUiBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn dimensions(mut self, width: u16, height: u16) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    pub fn remote_workspace(mut self) -> Self {
        self.session = self.session.remote_workspace();
        self
    }

    pub fn working_dir(mut self, working_dir: impl Into<PathBuf>) -> Self {
        self.session = self.session.working_dir(working_dir);
        self
    }

    pub fn prompt_capabilities(mut self, capabilities: acp::PromptCapabilities) -> Self {
        self.session = self.session.prompt_capabilities(capabilities);
        self
    }

    pub fn config_options(mut self, options: Vec<acp::SessionConfigOption>) -> Self {
        self.session = self.session.config_options(options);
        self
    }

    pub fn auth_methods(mut self, methods: Vec<acp::AuthMethod>) -> Self {
        self.session = self.session.auth_methods(methods);
        self
    }

    pub fn settings(mut self, settings: UiSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn workspace_status(mut self, workspace_status: WorkspaceStatus) -> Self {
        self.session = self.session.workspace_status(workspace_status);
        self
    }

    pub fn git(mut self, git: FakeGit) -> Self {
        self.git = git;
        self
    }

    /// Overrides the capabilities wholesale, for tests that care about metadata
    /// the individual toggles do not cover.
    pub fn session_capabilities(mut self, capabilities: acp::SessionCapabilities) -> Self {
        self.session = self.session.session_capabilities(capabilities);
        self
    }

    pub fn prompt_search(mut self) -> Self {
        self.session = self.session.prompt_search();
        self
    }

    pub fn session_preview(mut self) -> Self {
        self.session = self.session.session_preview();
        self
    }

    pub fn workspace_move(mut self) -> Self {
        self.session = self.session.workspace_move();
        self
    }

    /// Builds the whole UI scenario.
    pub fn build(self) -> TestUi {
        let app = App::new(self.app_config());
        let backend = TestBackend::new(self.width, self.height);
        self.finish(app, backend)
    }

    pub fn build_from_session(self, session: Session) -> (TestUi, UnboundedReceiver<AcpEvent>) {
        let (app, events, _) = App::from_session(session, self.settings.clone());
        let backend = TestBackend::new(self.width, self.height);
        let mut ui = self.finish(app, backend);
        ui.executor.record(ui.app.take_commands());
        (ui, events)
    }

    fn finish<B: Backend>(self, app: App, backend: B) -> TestUi<B>
    where
        B::Error: std::fmt::Debug,
    {
        TestUi {
            app,
            renderer: Renderer::new(),
            terminal: test_terminal(backend),
            executor: FakeExecutor::with_git(self.git),
            agent: FakeAgent::default(),
            opened_urls: self.opened_urls,
        }
    }

    fn app_config(&self) -> AppConfig {
        AppConfig {
            session: self.session.build(),
            settings: self.settings.clone(),
            browser_opener: {
                let opened = self.opened_urls.clone();
                Arc::new(move |url: &str| {
                    opened.lock().unwrap().push(url.to_string());
                    Ok(())
                }) as BrowserOpener
            },
            clipboard_writer: Arc::new(|_| Ok(())),
        }
    }
}

/// Whether any cell drawn with `symbol` satisfies `predicate`.
pub fn has_cell(buffer: &Buffer, symbol: &str, predicate: impl Fn(&Cell) -> bool) -> bool {
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            if let Some(cell) = buffer.cell((x, y))
                && cell.symbol() == symbol
                && predicate(cell)
            {
                return true;
            }
        }
    }
    false
}

pub fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// How many rows contain at least one cell with `background`.
pub fn rows_with_background(buffer: &Buffer, background: ratatui::style::Color) -> usize {
    (buffer.area.top()..buffer.area.bottom())
        .filter(|&y| row_cells(buffer, y).iter().any(|cell| cell.bg == background))
        .count()
}

pub fn row_cells(buffer: &Buffer, y: u16) -> Vec<&Cell> {
    (buffer.area.left()..buffer.area.right()).filter_map(|x| buffer.cell((x, y))).collect()
}

pub fn row_containing(buffer: &Buffer, needle: &str) -> Option<u16> {
    (buffer.area.top()..buffer.area.bottom()).find(|&y| row_text(buffer, y).contains(needle))
}

pub fn buffer_text(buffer: &Buffer) -> String {
    let mut out = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        for x in buffer.area.left()..buffer.area.right() {
            out.push_str(buffer.cell((x, y)).map_or(" ", Cell::symbol));
        }
        out.push('\n');
    }
    out
}

/// Asserts `buffer`'s visible text matches `expected` row-by-row after trimming
/// trailing spaces, panicking on the first mismatched line with the full buffer
/// dumped.
pub fn assert_buffer_eq<S: AsRef<str>>(buffer: &Buffer, expected: &[S]) {
    let actual_lines: Vec<String> =
        (buffer.area.top()..buffer.area.bottom()).map(|y| row_text(buffer, y).trim_end().to_string()).collect();
    for index in 0..actual_lines.len().max(expected.len()) {
        let actual_line = actual_lines.get(index).map_or("", String::as_str);
        let expected_line = expected.get(index).map_or("", AsRef::as_ref).trim_end();
        assert_eq!(
            actual_line,
            expected_line,
            "line {index} mismatch:\n  expected: {expected_line:?}\n  actual:   {actual_line:?}\n\nfull buffer:\n{}",
            actual_lines.join("\n")
        );
    }
}

pub fn row_text(buffer: &Buffer, y: u16) -> String {
    (buffer.area.left()..buffer.area.right()).map(|x| buffer.cell((x, y)).map_or(" ", Cell::symbol)).collect()
}

/// The shape of message [`TestUi::stream_message`] streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamContent {
    /// Flowing paragraphs, so finalization sealing runs as blank lines land and
    /// the open item stays small.
    Prose,
    /// One fenced Rust code block. No blank line outside the fence can finalize
    /// it while it streams, so the open item grows to the whole message — the
    /// shape that keeps re-rendering everything received so far.
    CodeBlock,
    /// A thinking stream: line-per-sentence thought chunks, which drive the
    /// progress band's activity rather than appending to the conversation.
    Thought,
}

pub fn session_update(update: acp::SessionUpdate) -> AcpEvent {
    acp::UpdateSessionNotification::new(SessionId::new("test-session"), update).into()
}

pub fn context_cleared() -> AcpEvent {
    AcpEvent::ContextCleared(ContextClearedParams { session_id: SessionId::new("test-session") })
}

pub fn compaction_update(id: &str, status: acp::CompactionStatus) -> AcpEvent {
    session_update(acp::SessionUpdate::CompactionUpdate(acp::CompactionUpdate::new(id, status)))
}

pub fn text_chunk(text: &str) -> AcpEvent {
    text_chunk_with_id("assistant", text)
}

pub fn text_chunk_with_id(message_id: &str, text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new(text)),
        message_id.to_string(),
    )))
}

pub fn thought_chunk(text: &str) -> AcpEvent {
    thought_chunk_with_id("thought", text)
}

pub fn thought_chunk_with_id(message_id: &str, text: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
        acp::ContentBlock::Text(acp::TextContent::new(text)),
        message_id.to_string(),
    )))
}

pub fn text_diff(path: &str, old: &str, new: &str) -> acp::Diff {
    let diff_text = git_patch_from_texts(path, Some(old), Some(new)).expect("valid text diff");
    acp::Diff::new(vec![acp::DiffChange::modify(acp::AbsolutePath::new(path))])
        .with_patch(diff_text.map(acp::DiffPatch::new))
}

pub fn sub_agent_progress(parent: &str, task: &str, agent: &str, event: SubAgentEvent) -> AcpEvent {
    AcpEvent::SubAgentProgress(SubAgentProgressParams {
        session_id: SessionId::new("test-session"),
        parent_tool_id: parent.to_string(),
        task_id: task.to_string(),
        agent_name: agent.to_string(),
        event,
    })
}

pub fn sub_agent_tool_update(parent: &str, task: &str, agent: &str, update: ToolCallUpdate) -> AcpEvent {
    sub_agent_progress(parent, task, agent, SubAgentEvent::ToolCallUpdate(Box::new(update)))
}

pub fn tool_completed(id: &str) -> AcpEvent {
    tool_status(id, acp::ToolCallStatus::Completed)
}

pub fn tool_status(id: &str, status: acp::ToolCallStatus) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(id.to_string()).status(status)))
}

pub fn numbered_lines(prefix: &str, count: usize) -> String {
    let mut lines = String::new();
    for index in 0..count {
        let _ = write!(lines, "{prefix}-{index}\n\n");
    }
    lines
}

pub fn render_surface<R>(mut buffer: Buffer, render: impl FnOnce(&mut Buffer, &mut DrawContext<'_>) -> R) -> Buffer {
    let theme = Theme::default();
    let mut highlighter = SyntaxHighlighter::new();
    let mut cx = DrawContext { theme: &theme, highlighter: &mut highlighter, theme_generation: Generation::default() };
    render(&mut buffer, &mut cx);
    buffer
}

pub fn git_diff_screen(files: Vec<FileDiff>) -> GitDiffScreen {
    let document = DiffDocument { repo_root: "/workspace".into(), files };
    let mut screen = GitDiffScreen::new();
    screen.install(&ClientState {
        connection: ConnectionState::Connected,
        snapshot: Some(Arc::new(DiffSnapshot { scope: DiffScope::Both, document: Arc::new(document) })),
        ..ClientState::default()
    });
    screen
}

pub fn chunk_message(message: &str, chunk_bytes: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut rest = message;
    while !rest.is_empty() {
        let mut end = rest.len().min(chunk_bytes);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        chunks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    chunks
}

fn assert_text_contains(surface: &str, text: &str, needle: &str, expected: bool) {
    let should = if expected { "should" } else { "should not" };
    assert!(text.contains(needle) == expected, "{surface} {should} contain {needle:?}:\n{text}");
}

/// The terminal a scenario draws into: the inline viewport sized from the
/// backend, exactly as the real event loop enters it.
fn test_terminal<B: Backend>(backend: B) -> Terminal<B>
where
    B::Error: std::fmt::Debug,
{
    let height = backend.size().unwrap().height;
    Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(inline_viewport_height(height)) })
        .unwrap()
}

/// What the inline viewport shows: `terminal.get_frame().area()` clipped out of
/// the backend's full screen buffer.
fn viewport_buffer<B: Backend + Borrow<TestBackend>>(terminal: &mut Terminal<B>) -> Buffer {
    let area = terminal.get_frame().area();
    let backend: &TestBackend = terminal.backend().borrow();
    crop(backend.buffer(), area, 0)
}

/// Content Ratatui's `insert_before` committed above the inline viewport.
fn history_buffer<B: Backend + Borrow<TestBackend>>(terminal: &mut Terminal<B>) -> Buffer {
    let viewport_top = terminal.get_frame().area().top();
    let backend: &TestBackend = terminal.backend().borrow();
    let mut history = backend.scrollback().clone();
    let above_viewport = Rect { height: viewport_top, ..backend.buffer().area };
    history.merge(&crop(backend.buffer(), above_viewport, history.area.height));
    history
}

fn conversation_buffer<B: Backend + Borrow<TestBackend>>(terminal: &mut Terminal<B>) -> Buffer {
    let mut conversation = history_buffer(terminal);
    let viewport = terminal.get_frame().area();
    let backend: &TestBackend = terminal.backend().borrow();
    conversation.merge(&crop(backend.buffer(), viewport, conversation.area.height));
    conversation
}

fn crop(buffer: &Buffer, area: Rect, y: u16) -> Buffer {
    let mut cropped = Buffer::empty(Rect { x: 0, y, ..area });
    for position in area.positions() {
        cropped[(position.x - area.x, position.y - area.y + y)] = buffer[position].clone();
    }
    cropped
}

const SEED_PROSE: &str = "\
Examining the request. The module guards its invariants behind a shared handle,
so the fix has to land on the writer side rather than at each call site. I will
rework the boundary so retries cannot observe a torn update, then cover the
regression with a test that fails on the current code.

";

const SEED_CODE_BLOCK: &str = "\
```rust
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::with_capacity(incoming.len());
    for delta in incoming {
        if !state.accepts(&delta) {
            continue;
        }
        state.apply(&delta);
        applied.push(delta);
    }
    state.commit(applied)
}
```

";

const SEED_CLOSING: &str = "\
Done — the writer now retries atomically and the regression test covers the
torn window.

";

const SEED_DIFF_BEFORE: &str = "\
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::new();
    for delta in incoming {
        state.apply(&delta);
    }
    state.commit(Vec::new())
}
";

const SEED_DIFF_AFTER: &str = "\
fn reconcile(state: &mut State, incoming: Vec<Delta>) -> Outcome {
    let mut applied = Vec::with_capacity(incoming.len());
    for delta in incoming {
        state.apply(&delta);
        applied.push(delta);
    }
    state.commit(applied)
}
";

fn seed_bash_tool(id: &str) -> AcpEvent {
    let mut tool_call = ToolCallUpdate::new(id.to_string()).title(format!("Run {id}"));
    tool_call.name = MaybeUndefined::Value("bash".into());
    tool_call.raw_input = MaybeUndefined::Value(json!({ "command": "cargo test --module writer" }));
    session_update(SessionUpdate::ToolCallUpdate(tool_call))
}

fn seed_edit_tool(id: &str, turn: usize) -> AcpEvent {
    session_update(SessionUpdate::ToolCallUpdate(
        ToolCallUpdate::new(id.to_string()).title(format!("Editing src/module_{turn}.rs")),
    ))
}

fn seed_spawn_tool(id: &str) -> AcpEvent {
    let mut tool_call = ToolCallUpdate::new(id.to_string()).title(format!("Spawning sub-agents ({id})"));
    tool_call.name = MaybeUndefined::Value("spawn_subagent".into());
    session_update(SessionUpdate::ToolCallUpdate(tool_call))
}

fn seed_tool_diff(id: &str, turn: usize) -> AcpEvent {
    let diff = text_diff(&format!("/src/module_{turn}.rs"), SEED_DIFF_BEFORE, SEED_DIFF_AFTER);
    session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(id.to_string())
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Completed),
    ))
}

fn prose_message(total_bytes: usize) -> String {
    let mut message = String::new();
    let mut sentence = 0;
    while message.len() < total_bytes {
        for _ in 0..4 {
            let _ =
                write!(message, "Sentence {sentence} carries ordinary words so wrapping and parsing do real work. ");
            sentence += 1;
        }
        message.push_str("\n\n");
    }
    message
}

fn code_block_message(total_bytes: usize) -> String {
    let mut message = String::from("```rust\n");
    let mut line = 0;
    while message.len() < total_bytes {
        let _ = writeln!(message, "let value_{line} = state.reconcile(incoming[{line}]).expect(\"delta accepted\");");
        line += 1;
    }
    message.push_str("```\n");
    message
}

fn thought_message(total_bytes: usize) -> String {
    let mut message = String::new();
    let mut step = 0;
    while message.len() < total_bytes {
        let _ = writeln!(message, "Considering step {step} of the plan before acting on it.");
        step += 1;
    }
    message
}
