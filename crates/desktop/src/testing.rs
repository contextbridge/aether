use crate::agents::{Agents, CommandLine, Endpoint, parse_command_line};
use crate::clock::Clock;
use crate::history::History;
use crate::platform::{ClipboardMedia, Notification, PathPrompt, Platform, PlatformFuture};
use crate::registry::{self, Registry, STATE_FILE};
use crate::remotes::{Acp, Fleet, InstanceState, RemoteSpec};
use crate::target::{ConnectError, Transport, TransportFuture, client_info};
use crate::threads::{Thread, ThreadManager, ThreadStatus};
use crate::{ActiveTheme, DesktopApp, DesktopConfig, cli::DEFAULT_AGENT_COMMAND, window_size};
use acp_utils::testing::{Channel, FakeAgent, FakeAgentRequests, QueuedTurns};
use aether_sessions::SessionStore;
use aether_sessions::testing::{TestStore, session_meta, user_message};
use chrono::{DateTime, Utc};
use clankerdiff_remote::RemotesFile;
use clankerdiff_remote::testing::FakeRemotes;
use clankerdiff_rouge::ui::testing::{TestError, TestUi, component_test};
use clankerdiff_rouge_shell::rouge::core::{
    Chord, Clipboard, FileDropPhase, Key, KeyEvent, Point, PointerButton, Rect, Size, WindowCommand, root_font_size,
    window_focused,
};
use clankerdiff_rouge_shell::rouge::reactive::{Entered, ReadSignal, Runtime as Reactive, Signal, set_tokio};
use clankerdiff_ui_core::ReviewTheme;
use client_core::connection::Connection;
use client_core::testing::{FakeGit, git_diff_agent};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::task::{AbortHandle, JoinHandle};

pub const FAKE_AGENT_LOCK_FILE: &str = "agent.lock";

pub struct DesktopHarnessBuilder {
    window_size: Option<(f32, f32)>,
    connector: FakeConnector,
    projects: Vec<PathBuf>,
    sessions: TestStore,
    saved_sessions: bool,
    state_dir: Option<PathBuf>,
    remotes: Vec<RemoteSpec>,
    now: DateTime<Utc>,
}

#[derive(Clone, Default)]
pub struct FakeConnector {
    state: Arc<Mutex<FakeConnectorState>>,
    remotes: FakeRemotes,
}

#[derive(Clone, Default)]
pub struct FakePlatform {
    state: Rc<RefCell<FakePlatformState>>,
}

pub struct DesktopHarness {
    ui: TestUi,
    app: DesktopApp,
    focused: ReadSignal<bool>,
    root_font_size: Signal<f32>,
    theme: ActiveTheme,
    platform: FakePlatform,
    connector: FakeConnector,
    sessions: TestStore,
    remotes_dir: TempDir,
    flush: Option<JoinHandle<io::Result<()>>>,
    runtime: Option<Runtime>,
    _entered: Entered,
}

pub fn launching_remote(name: &str, root: impl Into<PathBuf>) -> RemoteSpec {
    let command = |words: &[&str]| Some(words.iter().map(|word| (*word).to_owned()).collect());
    RemoteSpec {
        list: command(&["list"]),
        launch: command(&["launch"]),
        suspend: command(&["suspend", "${ID}"]),
        resume: command(&["resume", "${ID}"]),
        stop: command(&["stop", "${ID}"]),
        ..static_remote(name, root)
    }
}

pub fn static_remote(name: &str, root: impl Into<PathBuf>) -> RemoteSpec {
    let root = root.into();
    let cwd = root.to_string_lossy().into_owned();
    RemoteSpec {
        dir: Some(root),
        acp: Some(Acp { cwd: Some(cwd), ..Acp::stdio(vec!["agent".to_owned(), "${ID}".to_owned()]) }),
        ..RemoteSpec::named(name)
    }
}

impl Default for DesktopHarnessBuilder {
    fn default() -> Self {
        Self {
            window_size: None,
            connector: FakeConnector::default(),
            projects: Vec::new(),
            sessions: TestStore::new(),
            saved_sessions: false,
            state_dir: None,
            remotes: Vec::new(),
            now: parse_time(DEFAULT_NOW),
        }
    }
}

impl DesktopHarnessBuilder {
    pub fn window_size(mut self, width: f32, height: f32) -> Self {
        self.window_size = Some((width, height));
        self
    }

    pub fn now(mut self, now: &str) -> Self {
        self.now = parse_time(now);
        self
    }

    pub fn agent(self, cwd: impl Into<PathBuf>, agent: FakeAgent) -> Self {
        let cwd = cwd.into();
        self.script(&cwd, agent).project(cwd)
    }

    pub fn script(self, cwd: impl Into<PathBuf>, agent: FakeAgent) -> Self {
        self.connector.script(cwd, agent);
        self
    }

    pub fn git(self, cwd: impl Into<PathBuf>, git: FakeGit) -> Self {
        self.connector.serve_git(cwd, git);
        self
    }

    pub fn remote(mut self, remote: RemoteSpec) -> Self {
        self.remotes.push(remote);
        self
    }

    pub fn failing(self, name: &str, problem: &str) -> Self {
        self.connector.remotes().fail(name, problem);
        self
    }

    pub fn instance(self, remote: &str, id: &str, state: InstanceState, expires_at: Option<&str>) -> Self {
        let expires_at = expires_at.map(|time| SystemTime::from(parse_time(time)));
        self.connector.remotes().add(remote, id, state, expires_at);
        self
    }

    pub fn project(mut self, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        if !self.projects.contains(&root) {
            self.projects.push(root);
        }
        self
    }

    pub fn session<'a>(
        mut self,
        session_id: &str,
        cwd: &str,
        created_at: &str,
        prompt: impl Into<Option<&'a str>>,
    ) -> Self {
        save_session(&self.sessions, session_id, cwd, created_at, prompt.into());
        self.saved_sessions = true;
        self
    }

    pub fn state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
    }

    pub fn start(self, cwd: impl Into<PathBuf>) -> (DesktopHarness, Thread) {
        let mut harness = self.build();
        let thread = harness.start_thread(cwd);
        (harness, thread)
    }

    pub fn build(self) -> DesktopHarness {
        let entered = Reactive::new().enter();
        let runtime =
            Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("start the test Tokio runtime");
        let (saved, flush) = registry::load(self.state_dir.map(|dir| dir.join(STATE_FILE)), runtime.handle());
        saved.update(|state| {
            let before = state.projects.len();
            for root in &self.projects {
                state.add_project(root.clone());
            }
            state.projects.len() != before
        });
        let (width, height) = self.window_size.unwrap_or_else(|| {
            let size = window_size(&saved.state());
            (size.width, size.height)
        });
        let remotes_dir = tempfile::tempdir().expect("a directory for the remotes file");
        let remotes_file = remotes_dir.path().join("remotes.json");
        let file = serde_json::to_string(&RemotesFile { remotes: self.remotes }).expect("the remotes serialize");
        fs::write(&remotes_file, file).expect("the remotes file is written");
        let platform = FakePlatform::default();
        let config = DesktopConfig {
            transport: Arc::new(self.connector.clone()),
            clock: Arc::new(FakeClock(self.now)),
            agents: Agents::new(
                parse_command_line(DEFAULT_AGENT_COMMAND).expect("the default agent command parses"),
                None,
            ),
            remotes: Arc::new(self.connector.remotes().clone()),
            remotes_file: Some(remotes_file),
            sessions: Some(SessionStore::from_path(self.sessions.path().to_path_buf())),
            registry: saved,
            platform: Rc::new(platform.clone()),
        };
        let handle = runtime.handle().clone();
        let (mut ui, (app, focused, root_font_size, theme)) =
            component_test().size(width, height).build_with(move || {
                set_tokio(handle);
                let app = DesktopApp::new(config);
                (app.view(), (app, window_focused(), root_font_size(), ActiveTheme::current()))
            });
        ui.set_clipboard(platform.clone());
        let mut harness = DesktopHarness {
            ui,
            app,
            focused,
            root_font_size,
            theme,
            platform,
            connector: self.connector,
            sessions: self.sessions,
            remotes_dir,
            flush: Some(flush),
            runtime: Some(runtime),
            _entered: entered,
        };
        harness.advance();
        if self.saved_sessions {
            let history = harness.history();
            harness.run_until(|_| !history.with_projects(<[_]>::is_empty));
        }
        let fleet = harness.fleet();
        harness.run_until(|_| fleet.with_inventory(clankerdiff_remote::Inventory::has_listed));
        harness
    }
}

impl FakeConnector {
    pub fn script(&self, cwd: impl Into<PathBuf>, agent: FakeAgent) {
        self.lock().scripts.entry(cwd.into()).or_default().push_back(agent);
    }

    pub fn serve_git(&self, cwd: impl Into<PathBuf>, git: FakeGit) {
        self.lock().gits.insert(cwd.into(), git);
    }

    pub fn live_agents(&self, cwd: impl AsRef<Path>) -> usize {
        self.lock().live.get(cwd.as_ref()).map_or(0, Vec::len)
    }

    pub fn remotes(&self) -> &FakeRemotes {
        &self.remotes
    }

    pub fn kill_agents(&self, cwd: impl AsRef<Path>) {
        let agents = self.lock().live.remove(cwd.as_ref()).unwrap_or_default();
        for agent in agents {
            agent.task.abort();
        }
    }

    fn serve(&self, cwd: &Path, agent: FakeAgent, transport: Channel) {
        let served = self.clone();
        let served_cwd = cwd.to_path_buf();
        let mut state = self.lock();
        let id = state.next_agent;
        state.next_agent += 1;
        let git = state.gits.get(cwd).cloned();
        let task = tokio::spawn(async move {
            let _ = match git {
                Some(git) => git_diff_agent(agent, git).connect_to(transport).await,
                None => agent.agent().connect_to(transport).await,
            };
            served.lock().disconnected(&served_cwd, id);
        });
        state.live.entry(cwd.to_path_buf()).or_default().push(ServedAgent { id, task: task.abort_handle() });
    }

    fn lock(&self) -> MutexGuard<'_, FakeConnectorState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Transport for FakeConnector {
    fn stdio<'a>(&'a self, line: &'a CommandLine, dir: &'a Path) -> TransportFuture<'a, Connection> {
        Box::pin(async move {
            let agent = self.lock().take(dir).ok_or_else(|| ConnectError::Spawn {
                program: line.command().display().to_string(),
                source: io::Error::new(io::ErrorKind::NotFound, "no fake agent is scripted for this directory"),
            })?;
            let (agent_transport, client_transport) = Channel::duplex();
            self.serve(dir, agent, agent_transport);
            Ok(Connection::connect_to(client_info(), client_transport).await?)
        })
    }

    fn websocket<'a>(&'a self, endpoint: &'a Endpoint, _: Duration) -> TransportFuture<'a, Connection> {
        Box::pin(async move {
            Err(ConnectError::Spawn {
                program: endpoint.url.clone(),
                source: io::Error::new(io::ErrorKind::Unsupported, "no fake agent is served over a WebSocket"),
            })
        })
    }

    fn run<'a>(&'a self, _: &'a CommandLine, _: &'a Path) -> TransportFuture<'a, Vec<u8>> {
        Box::pin(std::future::pending())
    }
}

impl FakePlatform {
    pub fn notifications(&self) -> Vec<Notification> {
        self.state.borrow().notifications.clone()
    }

    pub fn click_notification(&self, tag: &str) {
        if let Some(clicked) = self.state.borrow_mut().clicks.remove(tag) {
            let _ = clicked.send(true);
        }
    }

    pub fn path_prompts(&self) -> Vec<PathPrompt> {
        self.state.borrow().prompts.iter().map(|(prompt, _)| prompt.clone()).collect()
    }

    pub fn answer_path_prompt(&self, paths: Option<Vec<PathBuf>>) -> Option<PathPrompt> {
        let (prompt, answer) = self.state.borrow_mut().prompts.pop_front()?;
        let _ = answer.send(paths);
        Some(prompt)
    }

    pub fn opened(&self) -> Vec<String> {
        self.state.borrow().opened.clone()
    }

    pub fn copy_text(&self, text: &str) {
        self.copy(Some(text.to_owned()), None);
    }

    pub fn copy_image(&self, png: &[u8]) {
        self.copy(None, Some(ClipboardMedia::Png(png.to_vec())));
    }

    pub fn copy_files(&self, paths: Vec<PathBuf>) {
        let names: Vec<_> =
            paths.iter().filter_map(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()).collect();
        self.copy(Some(names.join("\n")), Some(ClipboardMedia::Files(paths)));
    }

    pub fn copied_text(&self) -> Option<String> {
        self.state.borrow().clipboard_text.clone()
    }

    fn copy(&self, text: Option<String>, media: Option<ClipboardMedia>) {
        let mut state = self.state.borrow_mut();
        state.clipboard_text = text;
        state.clipboard_media = media;
    }
}

impl Clipboard for FakePlatform {
    fn read(&mut self) -> Option<String> {
        self.copied_text()
    }

    fn write(&mut self, text: &str) {
        self.copy_text(text);
    }
}

impl Platform for FakePlatform {
    fn notify(&self, notification: Notification) -> PlatformFuture<bool> {
        let (clicked, response) = oneshot::channel();
        let mut state = self.state.borrow_mut();
        state.clicks.insert(notification.tag.clone(), clicked);
        state.notifications.push(notification);
        Box::pin(async move { response.await.unwrap_or(false) })
    }

    fn pick_paths(&self, prompt: PathPrompt) -> PlatformFuture<Option<Vec<PathBuf>>> {
        let (answer, response) = oneshot::channel();
        self.state.borrow_mut().prompts.push_back((prompt, answer));
        Box::pin(async move { response.await.ok().flatten() })
    }

    fn open(&self, target: &str) {
        self.state.borrow_mut().opened.push(target.to_owned());
    }

    fn clipboard_media(&self) -> Option<ClipboardMedia> {
        self.state.borrow().clipboard_media.clone()
    }
}

impl DesktopHarness {
    pub fn alpha(agent: FakeAgent) -> (Self, Thread) {
        DesktopHarnessBuilder::default().agent("/alpha", agent).start("/alpha")
    }

    pub fn app(&self) -> DesktopApp {
        self.app
    }

    pub fn threads(&self) -> ThreadManager {
        self.app.threads()
    }

    pub fn selected_thread(&self) -> Option<Thread> {
        self.threads().selected()
    }

    pub fn new_thread(&mut self, cwd: impl Into<PathBuf>) -> Thread {
        let cwd = cwd.into();
        let threads = self.threads();
        let agent = threads.default_agent(&cwd);
        let thread = threads.new_thread(cwd, agent);
        self.advance();
        thread
    }

    pub fn start_thread(&mut self, cwd: impl Into<PathBuf>) -> Thread {
        let thread = self.new_thread(cwd);
        self.run_until(|_| thread.status() == ThreadStatus::Idle);
        thread
    }

    pub fn connected_thread(&mut self) -> Thread {
        let thread = self.selected_thread().expect("a thread is selected");
        self.run_until(|_| thread.status() == ThreadStatus::Idle);
        thread
    }

    pub fn start_turn(&mut self, thread: Thread, requests: &mut FakeAgentRequests, prompt: &str) -> QueuedTurns {
        thread.send(prompt.into());
        self.next(&mut requests.prompt);
        self.run_until(|_| {
            thread.status() == ThreadStatus::Running
                && thread.with_core(|core| core.queued_prompts().is_empty()).unwrap_or(false)
        });
        requests.turns.clone()
    }

    pub fn send(&mut self, thread: Thread, prompt: &str) {
        self.until_answered(thread, prompt, |harness| {
            harness.type_text(prompt);
            harness.press("enter");
        });
    }

    pub fn submit(&mut self, thread: Thread, prompt: &str) {
        self.until_answered(thread, prompt, |_| thread.send(prompt.into()));
    }

    pub fn fleet(&self) -> Fleet {
        self.app.fleet()
    }

    pub fn remotes(&self) -> &FakeRemotes {
        self.connector.remotes()
    }

    pub fn remotes_file(&self) -> PathBuf {
        self.remotes_dir.path().join("remotes.json")
    }

    pub fn history(&self) -> History {
        self.app.history()
    }

    pub fn registry(&self) -> Registry {
        self.app.registry()
    }

    pub fn projects(&self) -> Vec<PathBuf> {
        self.registry().with(|state| state.projects.clone())
    }

    pub fn connector(&self) -> &FakeConnector {
        &self.connector
    }

    pub fn platform(&self) -> &FakePlatform {
        &self.platform
    }

    pub fn save_session(&self, session_id: &str, cwd: &str, created_at: &str, prompt: Option<&str>) {
        save_session(&self.sessions, session_id, cwd, created_at, prompt);
    }

    pub fn ui(&mut self) -> &mut TestUi {
        &mut self.ui
    }

    pub fn bounds(&mut self, id: &str) -> Option<Rect> {
        self.advance();
        self.ui.bounds_of(id)
    }

    pub fn text(&mut self, id: &str) -> Option<String> {
        self.advance();
        self.ui.text_of(id)
    }

    pub fn shows(&mut self, id: &str, text: &str) -> bool {
        self.text(id).as_deref() == Some(text)
    }

    pub fn click(&mut self, id: &str) -> Result<(), TestError> {
        self.press_pointer(id, PointerButton::Primary)
    }

    pub fn click_at(&mut self, point: Point) {
        self.advance();
        self.ui.advance(CLICK_PAUSE);
        self.settle_pointer(point);
        self.ui.pointer_down(PointerButton::Primary);
        self.ui.pointer_up(PointerButton::Primary);
        self.advance();
    }

    pub fn theme(&mut self) -> ReviewTheme {
        self.advance();
        self.theme.review_signal().get_untracked()
    }

    pub fn right_click(&mut self, id: &str) -> Result<(), TestError> {
        self.press_pointer(id, PointerButton::Secondary)
    }

    pub fn hover(&mut self, id: &str) -> Result<(), TestError> {
        self.advance();
        let point = self.ui.center_of(id)?;
        self.settle_pointer(point);
        Ok(())
    }

    pub fn drag(&mut self, id: &str, to: Point) -> Result<(), TestError> {
        let from = self.ui.center_of(id)?;
        self.ui.pointer_moved(from);
        self.ui.pointer_down(PointerButton::Primary);
        for step in 1..=DRAG_STEPS {
            let progress = f32::from(step) / f32::from(DRAG_STEPS);
            self.ui.pointer_moved(Point::new(from.x + (to.x - from.x) * progress, from.y + (to.y - from.y) * progress));
            self.advance();
        }
        self.ui.pointer_up(PointerButton::Primary);
        self.advance();
        Ok(())
    }

    pub fn scroll(&mut self, id: &str, delta: Point) -> Result<(), TestError> {
        let point = self.ui.center_of(id)?;
        self.ui.pointer_moved(point);
        self.ui.scroll(delta);
        self.advance();
        Ok(())
    }

    pub fn click_nth(&mut self, id: &str, index: usize) -> Result<(), TestError> {
        self.advance();
        let target = self.ui.bounds_of_all(id).get(index).map(Rect::center);
        self.click_at(target.ok_or_else(|| TestError::NoTestId(format!("{id} #{index}")))?);
        Ok(())
    }

    pub fn scroll_into_view(&mut self, scroller: &str, id: &str) -> Result<(), TestError> {
        for _ in 0..MAX_SCROLLS {
            let view = self.bounds(scroller).ok_or_else(|| TestError::NoTestId(scroller.to_owned()))?;
            let target = self.bounds(id).ok_or_else(|| TestError::NoTestId(id.to_owned()))?;
            let delta = if target.max().y > view.max().y {
                view.max().y - target.max().y
            } else if target.origin.y < view.origin.y {
                view.origin.y - target.origin.y
            } else {
                return Ok(());
            };
            self.scroll(scroller, Point::new(0.0, delta))?;
        }
        panic!("`{id}` never scrolled into view in `{scroller}`");
    }

    pub fn type_text(&mut self, text: &str) {
        self.ui.type_text(text);
        self.advance();
    }

    pub fn press(&mut self, keys: &str) {
        for keys in keys.split_whitespace() {
            let chord = Chord::parse(keys).unwrap_or_else(|error| panic!("`{keys}` is not a chord: {error}"));
            let mut down = KeyEvent::new(chord.key.clone()).with_modifiers(chord.modifiers);
            let typed = match &chord.key {
                Key::Character(text) => Some(text.clone()),
                Key::Space => Some(" ".to_owned()),
                _ => None,
            };
            if let Some(typed) = typed.filter(|_| !chord.modifiers.command()) {
                down = down.with_text(typed);
            }
            self.ui.key_down(down);
            self.ui.key_up(KeyEvent::new(chord.key).with_modifiers(chord.modifiers));
            self.advance();
        }
    }

    pub fn drop_files(&mut self, on: &str, paths: Vec<PathBuf>) -> Result<(), TestError> {
        let point = self.ui.center_of(on)?;
        self.ui.pointer_moved(point);
        self.ui.file_drop(FileDropPhase::Hover, paths.clone());
        self.ui.file_drop(FileDropPhase::Drop, paths);
        self.advance();
        Ok(())
    }

    pub fn resize(&mut self, width: f32, height: f32) {
        self.ui.set_size(Size::new(width, height));
        self.advance();
    }

    pub fn rem_size(&mut self) -> f32 {
        self.advance();
        self.root_font_size.get_untracked()
    }

    pub fn advance(&mut self) {
        loop {
            self.ui.advance(0.0);
            let commands = self.ui.take_window_commands();
            if commands.is_empty() {
                return;
            }
            for command in commands {
                match command {
                    WindowCommand::Menu(item) => {
                        item.dispatch(&mut self.ui);
                    }
                    WindowCommand::Focus => self.ui.set_focused(true),
                    _ => {}
                }
            }
        }
    }

    pub fn run_until(&mut self, condition: impl Fn(&mut Self) -> bool) {
        loop {
            self.advance();
            if condition(self) {
                return;
            }
            std::thread::sleep(POLL);
        }
    }

    pub fn next<T>(&mut self, receiver: &mut UnboundedReceiver<T>) -> T {
        loop {
            self.advance();
            if let Ok(message) = receiver.try_recv() {
                return message;
            }
            std::thread::sleep(POLL);
        }
    }

    pub fn run_until_painted(&mut self, id: &str) {
        self.run_until(|harness| harness.ui.bounds_of(id).is_some());
    }

    pub fn activate(&mut self) {
        self.ui.set_focused(true);
        self.advance();
    }

    pub fn deactivate(&mut self) {
        self.ui.set_focused(false);
        self.advance();
    }

    pub fn is_active(&mut self) -> bool {
        self.advance();
        self.focused.get_untracked()
    }

    pub fn click_notification(&mut self, tag: &str) {
        self.platform.click_notification(tag);
        self.advance();
    }

    pub fn quit(mut self) {
        let (flush, runtime) = (self.flush.take(), self.runtime.take());
        drop(self);
        if let (Some(flush), Some(runtime)) = (flush, runtime) {
            let _ = runtime.block_on(flush);
            runtime.shutdown_background();
        }
    }

    fn press_pointer(&mut self, id: &str, button: PointerButton) -> Result<(), TestError> {
        self.advance();
        let point = self.ui.center_of(id)?;
        self.ui.advance(CLICK_PAUSE);
        self.settle_pointer(point);
        self.ui.pointer_down(button);
        self.ui.pointer_up(button);
        self.advance();
        Ok(())
    }

    fn settle_pointer(&mut self, point: Point) {
        self.ui.pointer_moved(point);
        self.ui.advance(HOVER_PAUSE);
        self.advance();
        self.ui.pointer_moved(point);
        self.advance();
    }

    fn until_answered(&mut self, thread: Thread, prompt: &str, send: impl FnOnce(&mut Self)) {
        let times_sent = |thread: Thread| {
            thread
                .with_core(|core| {
                    core.conversation()
                        .items()
                        .iter()
                        .filter(|item| item.text().is_some_and(|text| text == prompt))
                        .count()
                })
                .unwrap_or(0)
        };
        let expected = times_sent(thread) + 1;
        send(self);
        self.run_until(|_| thread.status() == ThreadStatus::Idle && times_sent(thread) == expected);
    }
}

impl Drop for DesktopHarness {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

struct FakeClock(DateTime<Utc>);

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

#[derive(Default)]
struct FakeConnectorState {
    scripts: HashMap<PathBuf, VecDeque<FakeAgent>>,
    gits: HashMap<PathBuf, FakeGit>,
    live: HashMap<PathBuf, Vec<ServedAgent>>,
    next_agent: u64,
}

struct ServedAgent {
    id: u64,
    task: AbortHandle,
}

#[derive(Default)]
struct FakePlatformState {
    notifications: Vec<Notification>,
    clicks: HashMap<String, oneshot::Sender<bool>>,
    prompts: VecDeque<(PathPrompt, oneshot::Sender<Option<Vec<PathBuf>>>)>,
    opened: Vec<String>,
    clipboard_text: Option<String>,
    clipboard_media: Option<ClipboardMedia>,
}

impl FakeConnectorState {
    fn take(&mut self, cwd: &Path) -> Option<FakeAgent> {
        self.scripts.get_mut(cwd)?.pop_front()
    }

    fn disconnected(&mut self, cwd: &Path, id: u64) {
        if let Some(live) = self.live.get_mut(cwd) {
            live.retain(|agent| agent.id != id);
        }
    }
}

fn save_session(store: &TestStore, session_id: &str, cwd: &str, created_at: &str, prompt: Option<&str>) {
    let mut meta = session_meta(session_id, created_at);
    meta.cwd = cwd.into();
    store.append_meta(session_id, &meta);
    if let Some(prompt) = prompt {
        store.append(session_id, &user_message(prompt));
    }
}

const DEFAULT_NOW: &str = "2026-01-07T12:00:00Z";
const DRAG_STEPS: u8 = 8;
const MAX_SCROLLS: usize = 100;
const POLL: Duration = Duration::from_millis(1);
const CLICK_PAUSE: f64 = 1.0;
const HOVER_PAUSE: f64 = 0.5;

fn parse_time(time: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(time).expect("an RFC 3339 time").to_utc()
}
