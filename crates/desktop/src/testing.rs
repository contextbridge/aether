use crate::agents::{Agents, CommandLine, Endpoint};
use crate::clock::Clock;
use crate::history::History;
use crate::registry::{Registry, STATE_FILE};
use crate::remotes::{Acp, Fleet, InstanceState, RemoteSpec};
use crate::target::{AgentConnector, ConnectError, Transport, TransportFuture, client_info};
use crate::threads::{Thread, ThreadManager, ThreadStatus};
use crate::ui::text_selector;
use crate::{DesktopApp, DesktopConfig, cli::DEFAULT_AGENT_COMMAND, init, window_options, window_size};
use acp_utils::testing::{Channel, FakeAgent, FakeAgentRequests, QueuedTurns};
use aether_sessions::SessionStore;
use aether_sessions::testing::{TestStore, session_meta, user_message};
use chrono::{DateTime, Utc};
use clankerdiff_gpui::ui::testing::WindowDriver;
use clankerdiff_gpui_shell::runtime;
use clankerdiff_remote::RemotesFile;
use clankerdiff_remote::testing::FakeRemotes;
use client_core::connection::Connection;
use client_core::testing::{FakeGit, git_diff_agent};
use gpui::{
    App, AppContext, Bounds, Entity, Pixels, Size, SystemNotificationResponse, TestAppContext, VisualTestContext,
    WindowHandle, point, px, size,
};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};
use tempfile::TempDir;
use tokio::runtime::{Builder, Runtime};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::{AbortHandle, yield_now};

pub const FAKE_AGENT_LOCK_FILE: &str = "agent.lock";

pub struct DesktopHarnessBuilder {
    window_size: Option<Size<Pixels>>,
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
        self.window_size = Some(size(px(width), px(height)));
        self
    }

    pub fn now(mut self, now: &str) -> Self {
        self.now = parse_time(now);
        self
    }

    pub fn connector(mut self, connector: FakeConnector) -> Self {
        self.connector = connector;
        self
    }

    pub fn agent(self, cwd: impl Into<PathBuf>, agent: FakeAgent) -> Self {
        let cwd = cwd.into();
        self.connector.script(&cwd, agent);
        self.project(cwd)
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

    pub fn start(self, cx: &mut TestAppContext, cwd: impl Into<PathBuf>) -> (DesktopHarness, Entity<Thread>) {
        let harness = self.build(cx);
        let thread = harness.start_thread(cx, cwd);
        (harness, thread)
    }

    pub fn build(self, cx: &mut TestAppContext) -> DesktopHarness {
        let runtime = Builder::new_current_thread().enable_all().build().expect("start the test Tokio runtime");
        cx.update(|cx| {
            runtime::install(cx, runtime.handle().clone());
            init(cx);
        });
        let registry = cx.new(|cx| Registry::load(self.state_dir.map(|dir| dir.join(STATE_FILE)), cx));
        Registry::update(&registry, cx, |state| {
            for root in self.projects {
                state.add_project(root);
            }
        });
        let window_size = self.window_size.unwrap_or_else(|| cx.read(|cx| window_size(&registry, cx)));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), window_size);
        let remotes_dir = tempfile::tempdir().expect("a directory for the remotes file");
        let remotes_file = remotes_dir.path().join("remotes.json");
        let file = serde_json::to_string(&RemotesFile { remotes: self.remotes }).expect("the remotes serialize");
        fs::write(&remotes_file, file).expect("the remotes file is written");
        let remotes = Arc::new(self.connector.remotes().clone());
        let config = DesktopConfig {
            connector: Arc::new(AgentConnector::new(remotes.clone(), Arc::new(self.connector.clone()))),
            clock: Arc::new(FakeClock(self.now)),
            agents: Agents::new(
                CommandLine::parse(DEFAULT_AGENT_COMMAND).expect("the default agent command parses"),
                None,
            ),
            remotes,
            remotes_file: Some(remotes_file),
            sessions: Some(SessionStore::from_path(self.sessions.path().to_path_buf())),
            registry,
        };
        let window = cx.update(|cx| {
            cx.open_window(window_options(bounds), |window, cx| cx.new(|cx| DesktopApp::new(config, window, cx)))
                .expect("open the gpui test window")
        });
        let harness = DesktopHarness {
            window,
            driver: WindowDriver::new(window),
            runtime,
            connector: self.connector,
            sessions: self.sessions,
            remotes_dir,
        };
        harness.activate(cx);
        if self.saved_sessions {
            let history = harness.history(cx);
            harness.run_until(cx, |cx| !history.read(cx).projects().is_empty());
        }
        let fleet = harness.fleet(cx);
        harness.run_until(cx, |cx| fleet.read(cx).inventory().has_listed());
        harness
    }
}

pub struct DesktopHarness {
    window: WindowHandle<DesktopApp>,
    driver: WindowDriver,
    runtime: Runtime,
    connector: FakeConnector,
    sessions: TestStore,
    remotes_dir: TempDir,
}

impl FakeConnector {
    pub fn scripted(cwd: impl Into<PathBuf>, agents: impl IntoIterator<Item = FakeAgent>) -> Self {
        let (connector, cwd) = (Self::default(), cwd.into());
        for agent in agents {
            connector.script(&cwd, agent);
        }
        connector
    }

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
                program: line.command.clone(),
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

impl Deref for DesktopHarness {
    type Target = WindowDriver;

    fn deref(&self) -> &WindowDriver {
        &self.driver
    }
}

impl DesktopHarness {
    pub fn alpha(cx: &mut TestAppContext, agent: FakeAgent) -> (Self, Entity<Thread>) {
        DesktopHarnessBuilder::default().agent("/alpha", agent).start(cx, "/alpha")
    }

    pub fn threads(&self, cx: &TestAppContext) -> Entity<ThreadManager> {
        cx.read(|cx| self.app(cx).threads().clone())
    }

    pub fn selected_thread(&self, cx: &TestAppContext) -> Option<Entity<Thread>> {
        cx.read(|cx| self.app(cx).threads().read(cx).selected().cloned())
    }

    pub fn new_thread(&self, cx: &mut TestAppContext, cwd: impl Into<PathBuf>) -> Entity<Thread> {
        let cwd = cwd.into();
        self.threads(cx).update(cx, |threads, cx| {
            let agent = threads.default_agent(&cwd, cx);
            threads.new_thread(cwd, agent, cx)
        })
    }

    pub fn start_thread(&self, cx: &mut TestAppContext, cwd: impl Into<PathBuf>) -> Entity<Thread> {
        let thread = self.new_thread(cx, cwd);
        self.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
        thread
    }

    pub fn connected_thread(&self, cx: &mut TestAppContext) -> Entity<Thread> {
        let thread = self.selected_thread(cx).expect("a thread is selected");
        self.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
        thread
    }

    pub fn start_turn(
        &self,
        cx: &mut TestAppContext,
        thread: &Entity<Thread>,
        requests: &mut FakeAgentRequests,
        prompt: &str,
    ) -> QueuedTurns {
        thread.update(cx, |thread, cx| thread.send(prompt.into(), cx));
        self.next(cx, &mut requests.prompt);
        self.run_until(cx, |cx| {
            let thread = thread.read(cx);
            thread.status() == ThreadStatus::Running
                && thread.core().is_some_and(|core| core.queued_prompts().is_empty())
        });
        requests.turns.clone()
    }

    pub fn send(&self, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
        self.until_answered(cx, thread, prompt, |harness, cx| {
            harness.simulate_input(cx, prompt);
            harness.simulate_keystrokes(cx, "enter");
        });
    }

    pub fn submit(&self, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
        self.until_answered(cx, thread, prompt, |_, cx| thread.update(cx, |thread, cx| thread.send(prompt.into(), cx)));
    }

    pub fn fleet(&self, cx: &TestAppContext) -> Entity<Fleet> {
        cx.read(|cx| self.app(cx).fleet().clone())
    }

    pub fn remotes(&self) -> &FakeRemotes {
        self.connector.remotes()
    }

    pub fn remotes_file(&self) -> PathBuf {
        self.remotes_dir.path().join("remotes.json")
    }

    pub fn history(&self, cx: &TestAppContext) -> Entity<History> {
        cx.read(|cx| self.app(cx).history().clone())
    }

    pub fn registry(&self, cx: &TestAppContext) -> Entity<Registry> {
        cx.read(|cx| self.app(cx).registry().clone())
    }

    pub fn projects(&self, cx: &TestAppContext) -> Vec<PathBuf> {
        self.registry(cx).read_with(cx, |registry, _| registry.state().projects.clone())
    }

    pub fn connector(&self) -> &FakeConnector {
        &self.connector
    }

    pub fn save_session(&self, session_id: &str, cwd: &str, created_at: &str, prompt: Option<&str>) {
        save_session(&self.sessions, session_id, cwd, created_at, prompt);
    }

    pub fn shows(&self, cx: &mut TestAppContext, selector: &str, text: &str) -> bool {
        let selector: &'static str = text_selector(selector, text).leak();
        self.bounds(cx, selector).is_some()
    }

    pub fn run_until(&self, cx: &mut TestAppContext, condition: impl Fn(&App) -> bool) {
        loop {
            cx.run_until_parked();
            if cx.read(&condition) {
                return;
            }
            self.runtime.block_on(yield_now());
        }
    }

    pub fn next<T>(&self, cx: &mut TestAppContext, receiver: &mut UnboundedReceiver<T>) -> T {
        self.run_until(cx, |_| !receiver.is_empty());
        receiver.try_recv().expect("a message is waiting")
    }

    pub fn run_until_painted(&self, cx: &mut TestAppContext, selector: &'static str) {
        while self.bounds(cx, selector).is_none() {
            self.runtime.block_on(yield_now());
        }
    }

    pub fn activate(&self, cx: &mut TestAppContext) {
        cx.update_window(*self.window, |_, window, _| window.activate_window()).expect("the test window is open");
        self.draw(cx);
    }

    pub fn deactivate(&self, cx: &mut TestAppContext) {
        VisualTestContext::from_window(*self.window, cx).deactivate_window();
        self.draw(cx);
    }

    pub fn enter_full_screen(&self, cx: &mut TestAppContext, width: f32, height: f32) {
        cx.update_window(*self.window, |_, window, _| window.toggle_fullscreen()).expect("the test window is open");
        VisualTestContext::from_window(*self.window, cx).simulate_resize(size(px(width), px(height)));
        self.draw(cx);
    }

    pub fn is_active(&self, cx: &mut TestAppContext) -> bool {
        cx.update_window(*self.window, |_, window, _| window.is_window_active()).expect("the test window is open")
    }

    pub fn click_notification(&self, cx: &mut TestAppContext, tag: &str) {
        cx.simulate_system_notification_response(SystemNotificationResponse {
            tag: tag.to_owned().into(),
            action_id: None,
        });
        self.draw(cx);
    }

    fn app<'a>(&self, cx: &'a App) -> &'a DesktopApp {
        self.window.read(cx).expect("the test window is open")
    }

    fn until_answered(
        &self,
        cx: &mut TestAppContext,
        thread: &Entity<Thread>,
        prompt: &str,
        send: impl FnOnce(&Self, &mut TestAppContext),
    ) {
        let times_sent = |cx: &App| {
            thread.read(cx).core().map_or(0, |core| {
                core.conversation().items().iter().filter(|item| item.text().is_some_and(|text| text == prompt)).count()
            })
        };
        let expected = cx.read(times_sent) + 1;
        send(self, cx);
        self.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle && times_sent(cx) == expected);
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

fn parse_time(time: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(time).expect("an RFC 3339 time").to_utc()
}
