//! Public testing harness for driving the application model without side effects.
//!
//! Prefer [`TestUi`] over reaching for [`App`], [`Renderer`], or a raw
//! `Terminal<TestBackend>`: it owns all of them and routes input, ACP events,
//! task settling, and drawing through the same seams the event loop uses.

use crate::app::message::Message;
use crate::app::{App, AppConfig};
use crate::command::{Command, HostCommand, HostCommandResult};
use crate::platform::Platform;
use crate::platform::terminal::inline_viewport_height;
pub use crate::renderer::RenderStats;
use crate::renderer::{DrawContext, Renderer};
use crate::runtime::{Host, Runtimes};
use crate::screens::git_diff::GitDiffScreen;
use crate::settings::{UiSettings, builtin_review_theme_choices};
use crate::surfaces::composer::ComposerLayout;
use crate::theme::Theme;
use crate::view::generation::Generation;
use crate::view::syntax::SyntaxHighlighter;
use acp_utils::client::{AcpEvent, connect_acp_client};
use acp_utils::notifications::{
    ContextClearedParams, PromptSearchParams, SessionPreviewParams, WorkspaceListParams, WorkspaceMoveParams,
};
pub use acp_utils::testing::sub_agent_tool_update;
use acp_utils::testing::{
    Channel, FakeAgent, FakeAgentRequests, announced, idle_state, initialize_request, message_chunk, on_session,
    running_state,
};
use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::schema::v2::{
    self as acp, CreateElicitationRequest, CreateElicitationResponse, SessionId, SessionUpdate, ToolCallUpdate,
};
use agent_client_protocol::{Error, JsonRpcRequest, JsonRpcResponse, Responder};
use clankerdiff_client::{ClientState, ConnectionState};
use clankerdiff_core::git_patch_from_texts;
use clankerdiff_ratatui::ThemeChoice;
use clankerdiff_ratatui::diff::{DiffDocument, FileDiff};
use client_core::composer::file_index::index_files;
use client_core::runtime::{Input, SessionRuntime};
use client_core::session_core::ForegroundOperation;
use client_core::testing::{FakeGit, SessionCoreConfigBuilder, git_diff_agent};
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use futures::FutureExt;
use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::{Position, Rect, Size};
use ratatui::{Terminal, TerminalOptions, Viewport};
use serde_json::json;
use std::borrow::Borrow;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, LocalSet};
use unicode_width::UnicodeWidthStr;

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
///
/// Construct through [`TestUi::new`], [`TestUi::with_dimensions`], or
pub struct TestUi<B: Backend = TestBackend> {
    app: App,
    renderer: Renderer,
    terminal: Terminal<B>,
    world: World,
    opened_urls: Arc<Mutex<Vec<String>>>,
    bells: Arc<AtomicUsize>,
}

impl<B: Backend> TestUi<B>
where
    B::Error: std::fmt::Debug,
{
    /// Builds a UI around an arbitrary backend (e.g. a recording backend that
    /// asserts on frame-level terminal commands). The app is the default
    /// scenario; configure anything else through [`TestUiBuilder`].
    pub fn with_backend(backend: B) -> Self {
        TestUiBuilder::new().finish(backend)
    }

    /// The app under test, for the read-only queries assertions are built from.
    /// Drive it through this harness rather than mutating it directly, so the
    pub fn app(&self) -> &App {
        &self.app
    }

    pub fn deliver(&mut self, message: Message) {
        let commands = self.app.update(message);
        if commands.is_empty() {
            return;
        }
        self.world.dispatch(commands);
        self.settle_tasks();
    }

    pub fn settle_tasks(&mut self) {
        self.world.settle(&mut self.app);
    }

    pub fn advance(&mut self, duration: Duration) {
        self.world.tokio.block_on(self.world.local.run_until(tokio::time::advance(duration)));
        self.settle_tasks();
    }

    pub fn deliver_host_result(&mut self, result: HostCommandResult) {
        self.deliver(Message::Host(Box::new(result)));
    }

    pub fn applied_themes(&mut self) -> Vec<UiSettings> {
        self.world.runtimes.host.applying.drain(..).map(|settings| *settings).collect()
    }

    pub fn agent(&mut self) -> &mut FakeAgentRequests {
        &mut self.world.agent
    }

    pub fn held<T: JsonRpcRequest>(&mut self) -> (T, Responder<T::Response>) {
        self.try_held::<T>().unwrap_or_else(|| panic!("the agent holds no {}", std::any::type_name::<T>()))
    }

    pub fn try_held<T: JsonRpcRequest>(&mut self) -> Option<(T, Responder<T::Response>)> {
        self.world.agent.held::<T>().try_recv().ok()
    }

    pub fn answer<T: JsonRpcRequest>(&mut self, response: Result<T::Response, Error>) -> T {
        let (request, responder) = self.held::<T>();
        self.respond(responder, response);
        request
    }

    pub fn respond<T: JsonRpcResponse>(&mut self, responder: Responder<T>, response: Result<T, Error>) {
        let _ = match response {
            Ok(response) => responder.respond(response),
            Err(error) => responder.respond_with_error(error),
        };
        self.settle_tasks();
    }

    pub fn complete_prompt(&mut self, stop_reason: acp::StopReason) {
        self.world.agent.turns.end(stop_reason);
        self.settle_tasks();
    }

    pub fn insert_queued_prompts(&mut self) {
        self.world.agent.turns.insert_waiting();
        self.settle_tasks();
    }

    pub fn reject_queued_prompts(&mut self, error: &Error) {
        self.world.agent.turns.reject_waiting(error);
        self.settle_tasks();
    }

    pub fn refuse_prompts(&mut self, error: Error) {
        self.world.agent.turns.refuse(error);
    }

    pub fn elicit(&mut self, request: CreateElicitationRequest) -> ElicitationReply {
        let reply = self.world.agent.turns.ask(request);
        self.settle_tasks();
        ElicitationReply(reply)
    }

    pub fn answer_to(&mut self, mut reply: ElicitationReply) -> CreateElicitationResponse {
        self.settle_tasks();
        reply.0.try_recv().expect("the question was answered").expect("the answer reached the agent")
    }

    pub fn quit_answering(self, reply: ElicitationReply) -> CreateElicitationResponse {
        let Self { app, world, .. } = self;
        drop(app);
        let ElicitationReply(reply) = reply;
        world
            .tokio
            .block_on(world.local.run_until(reply))
            .expect("the question was answered")
            .expect("the answer reached the agent")
    }

    pub fn disconnect_agent(&mut self) {
        self.world.connection.abort();
        self.settle_tasks();
    }

    pub fn workspace(&self) -> &Path {
        &self.world.root
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

    pub fn bells(&self) -> usize {
        self.bells.load(Ordering::Relaxed)
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
        self.deliver(Message::Session(Box::new(Input::Event(event))));
    }

    pub fn run_command(&mut self, command: &str) {
        self.type_text(command);
        self.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    }

    pub fn open_session_picker(&mut self, sessions: Vec<acp::SessionInfo>) {
        self.run_command("/resume");
        self.answer::<acp::ListSessionsRequest>(Ok(acp::ListSessionsResponse::new(sessions)));
    }

    pub fn begin_resume(&mut self, session_id: &str, cwd: &str) {
        self.open_session_picker(vec![acp::SessionInfo::new(session_id.to_string(), cwd)]);
        self.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    }

    pub fn tick(&mut self, now: Instant) {
        self.deliver(Message::Tick(now));
    }

    fn seed_sub_agent_tree(&mut self, turn: usize) {
        let parent = format!("seed-spawn-{turn}");
        self.acp_event(seed_spawn_tool(&parent));
        self.acp_event(tool_completed(&parent));
        for agent in ["explorer", "fixer"] {
            let session = format!("{parent}-{agent}");
            let grep = format!("{session}-grep");
            self.acp_event(sub_agent_started(&parent, &session, agent));
            self.acp_event(sub_agent_tool_update(
                session.as_str(),
                ToolCallUpdate::new(grep.clone())
                    .title("Grep")
                    .name("coding__grep")
                    .raw_input(json!({ "pattern": "torn update" })),
            ));
            self.acp_event(sub_agent_tool_update(
                session.as_str(),
                ToolCallUpdate::new(grep).status(acp::ToolCallStatus::Completed),
            ));
            self.acp_event(sub_agent_finished(&parent, &session, agent, acp::StopReason::EndTurn));
        }
    }
}

pub struct ElicitationReply(oneshot::Receiver<Result<CreateElicitationResponse, Error>>);

impl ElicitationReply {
    pub fn is_answered(&self) -> bool {
        !self.0.is_empty()
    }
}

struct World {
    tokio: tokio::runtime::Runtime,
    local: LocalSet,
    runtimes: Runtimes<FakeTuiHost>,
    agent: FakeAgentRequests,
    connection: JoinHandle<Result<(), Error>>,
    _workspace: TempDir,
    root: PathBuf,
}

impl World {
    fn dispatch(&mut self, commands: Vec<Command>) {
        let _runtime = self.tokio.enter();
        self.runtimes.dispatch(commands);
    }

    fn settle(&mut self, app: &mut App) {
        let Self { tokio, local, runtimes, .. } = self;
        tokio.block_on(local.run_until(async {
            let mut quiet_turns = 0;
            while quiet_turns < QUIET_TURNS
                || matches!(app.core().foreground(), ForegroundOperation::PreparingPrompt(_))
            {
                let message = match runtimes.host.results.pop_front() {
                    Some(result) => Some(Message::Host(Box::new(result))),
                    None => runtimes.session.next().now_or_never().map(|input| Message::Session(Box::new(input))),
                };
                let Some(message) = message else {
                    quiet_turns += 1;
                    tokio::task::yield_now().await;
                    continue;
                };
                quiet_turns = 0;
                runtimes.deliver(app, message);
            }
        }));
    }
}

#[derive(Default)]
struct FakeTuiHost {
    workspace: PathBuf,
    themes: Vec<String>,
    review_themes: Vec<ThemeChoice>,
    results: VecDeque<HostCommandResult>,
    applying: VecDeque<Box<UiSettings>>,
}

impl Host for FakeTuiHost {
    fn execute(&mut self, command: HostCommand) {
        match command {
            HostCommand::ListThemes => self.results.push_back(HostCommandResult::ThemesListed(self.themes.clone())),
            HostCommand::ListReviewThemes => {
                let mut choices = builtin_review_theme_choices();
                choices.extend(self.review_themes.iter().cloned());
                self.results.push_back(HostCommandResult::ReviewThemesListed(choices));
            }
            HostCommand::IndexFiles { root } => {
                let files = if root.starts_with(&self.workspace) { index_files(&root) } else { Vec::new() };
                self.results.push_back(HostCommandResult::FilesIndexed(files));
            }
            HostCommand::ApplyTheme { settings } => self.applying.push_back(settings),
        }
    }
}

fn recording_platform(opened_urls: &Arc<Mutex<Vec<String>>>, bells: &Arc<AtomicUsize>) -> Platform {
    let (opened_urls, bells) = (opened_urls.clone(), bells.clone());
    Platform {
        browser_opener: Arc::new(move |url: &str| {
            opened_urls.lock().unwrap().push(url.to_string());
            Ok(())
        }),
        clipboard_writer: Arc::new(|_| Ok(())),
        bell: Arc::new(move || {
            bells.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }),
    }
}

const QUIET_TURNS: usize = 64;

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

    /// Asserts the stitched conversation's visible text matches `expected` row-by-row.
    pub fn assert_conversation<S: AsRef<str>>(&mut self, expected: &[S]) {
        assert_buffer_eq(&self.conversation(), expected);
    }
}

pub struct TestUiBuilder {
    width: u16,
    height: u16,
    session: SessionCoreConfigBuilder,
    agent: Vec<Box<dyn FnOnce(FakeAgent) -> FakeAgent>>,
    settings: UiSettings,
    themes: Vec<String>,
    review_themes: Vec<ThemeChoice>,
    git: FakeGit,
    files: Vec<(PathBuf, Vec<u8>)>,
}

impl Default for TestUiBuilder {
    fn default() -> Self {
        Self {
            width: 40,
            height: 15,
            session: SessionCoreConfigBuilder::new(),
            agent: Vec::new(),
            settings: UiSettings::default(),
            themes: Vec::new(),
            review_themes: Vec::new(),
            git: FakeGit::default(),
            files: Vec::new(),
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

    pub fn session(mut self, describe: impl FnOnce(SessionCoreConfigBuilder) -> SessionCoreConfigBuilder) -> Self {
        self.session = describe(self.session);
        self
    }

    pub fn agent(mut self, script: impl FnOnce(FakeAgent) -> FakeAgent + 'static) -> Self {
        self.agent.push(Box::new(script));
        self
    }

    pub fn settings(mut self, settings: UiSettings) -> Self {
        self.settings = settings;
        self
    }

    pub fn themes(mut self, themes: &[&str]) -> Self {
        self.themes = themes.iter().map(|theme| (*theme).to_owned()).collect();
        self
    }

    pub fn review_themes(mut self, themes: Vec<ThemeChoice>) -> Self {
        self.review_themes = themes;
        self
    }

    pub fn git(mut self, git: FakeGit) -> Self {
        self.git = git;
        self
    }

    pub fn file(mut self, path: impl Into<PathBuf>, contents: impl AsRef<[u8]>) -> Self {
        self.files.push((path.into(), contents.as_ref().to_vec()));
        self
    }

    /// Builds the whole UI scenario.
    pub fn build(self) -> TestUi {
        let backend = TestBackend::new(self.width, self.height);
        self.finish(backend)
    }

    fn finish<B: Backend>(self, backend: B) -> TestUi<B>
    where
        B::Error: std::fmt::Debug,
    {
        let tokio = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .expect("the test runtime starts");
        let local = LocalSet::new();
        let workspace = TempDir::new().expect("the workspace is created");
        let root = workspace.path().canonicalize().expect("the workspace has a path");
        for (path, contents) in &self.files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().expect("a file has a parent")).expect("the directory is created");
            std::fs::write(path, contents).expect("the file is written");
        }
        let mut session = self.session.git_diff();
        if !self.files.is_empty() {
            session = session.working_dir(&root);
        }
        let (agent, requests) = self
            .agent
            .into_iter()
            .fold(session.agent(), |agent, script| script(agent))
            .hold::<acp::ListSessionsRequest>()
            .hold::<acp::ResumeSessionRequest>()
            .hold::<acp::NewSessionRequest>()
            .hold::<acp::LoginAuthRequest>()
            .hold::<SessionPreviewParams>()
            .hold::<PromptSearchParams>()
            .hold::<WorkspaceListParams>()
            .hold::<WorkspaceMoveParams>()
            .capture();
        let server = git_diff_agent(agent, self.git);
        let (connection, client) = tokio.block_on(local.run_until(async {
            let (agent_end, client_end) = Channel::duplex();
            let connection = tokio::task::spawn_local(server.connect_to(agent_end));
            let client = connect_acp_client(client_end, initialize_request()).await.expect("the agent connects");
            (connection, client)
        }));

        let opened_urls = Arc::new(Mutex::new(Vec::new()));
        let bells = Arc::new(AtomicUsize::new(0));
        let config = AppConfig {
            session: session.build(),
            settings: self.settings,
            platform: recording_platform(&opened_urls, &bells),
        };
        let host = FakeTuiHost {
            workspace: root.clone(),
            themes: self.themes,
            review_themes: self.review_themes,
            ..FakeTuiHost::default()
        };
        let mut ui = TestUi {
            app: App::new(config),
            renderer: Renderer::new(),
            terminal: test_terminal(backend),
            world: World {
                tokio,
                local,
                runtimes: Runtimes { session: SessionRuntime::new(client.handle, client.event_rx), host },
                agent: requests,
                connection,
                _workspace: workspace,
                root,
            },
            opened_urls,
            bells,
        };
        let commands = ui.app.take_commands();
        ui.world.dispatch(commands);
        ui.settle_tasks();
        ui
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
    on_session("test-session", update)
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
    session_update(message_chunk(message_id, text))
}

pub fn thought_chunk(text: &str) -> AcpEvent {
    thought_chunk_with_id("thought", text)
}

pub fn thought_chunk_with_id(message_id: &str, text: &str) -> AcpEvent {
    session_update(acp_utils::testing::thought_chunk(message_id, text))
}

pub fn text_diff(path: &str, old: &str, new: &str) -> acp::Diff {
    let diff_text = git_patch_from_texts(path, Some(old), Some(new)).expect("valid text diff");
    acp::Diff::new(vec![acp::DiffChange::modify(acp::AbsolutePath::new(path))])
        .with_patch(diff_text.map(acp::DiffPatch::new))
}

pub fn sub_agent_started(parent: &str, session: &str, agent: &str) -> AcpEvent {
    announced("test-session", parent, acp::SubagentUpdate::new(session).title(agent).state(running_state())).into()
}

pub fn sub_agent_finished(parent: &str, session: &str, agent: &str, stop_reason: acp::StopReason) -> AcpEvent {
    announced("test-session", parent, acp::SubagentUpdate::new(session).title(agent).state(idle_state(stop_reason)))
        .into()
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
    let document = DiffDocument { repo_root: "/workspace".into(), files, ..DiffDocument::default() };
    let mut screen = GitDiffScreen::new();
    screen.install(&ClientState {
        connection: ConnectionState::Connected,
        snapshot: Some(Arc::new(document)),
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
