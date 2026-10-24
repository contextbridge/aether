//! Integration-test support: opens the production [`DesktopApp`] in a gpui test window, with
//! in-memory agents in place of agent processes.

use crate::agents::{AgentCommand, Agents, Endpoint, EndpointSource};
use crate::clock::Clock;
use crate::history::{History, git_root};
use crate::registry::{Registry, STATE_FILE};
use crate::target::{ConnectError, ConnectMode, Connector, ConnectorFuture, InstanceId, SessionTarget, client_info};
use crate::threads::{Thread, ThreadManager, ThreadStatus};
use crate::{DesktopApp, DesktopConfig, cli::DEFAULT_AGENT_COMMAND, init, window_options, window_size};
use acp_utils::testing::{Channel, FakeAgent};
use aether_sessions::SessionStore;
use aether_sessions::testing::{TestStore, session_meta, user_message};
use chrono::{DateTime, Utc};
use clankerdiff_gpui::ui::testing::WindowDriver;
use clankerdiff_gpui_shell::runtime;
use client_core::session::{Connection, Session};
use client_core::testing::{FakeGit, FakeGitDiffServer};
use gpui::{
    App, AppContext, Bounds, Entity, Pixels, Size, SystemNotificationResponse, TestAppContext, VisualTestContext,
    WindowHandle, point, px, size,
};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::runtime::{Builder, Runtime};
use tokio::task::{AbortHandle, yield_now};

/// The moment a harness runs at unless given another: a week after 2026-01-01, so sessions saved
/// in the first week of January 2026 are recent.
pub const DEFAULT_NOW: &str = "2026-01-07T12:00:00Z";

/// The file the `fake-acp-agent` test binary holds an exclusive lock on, in its working
/// directory, for as long as it runs.
pub const FAKE_AGENT_LOCK_FILE: &str = "agent.lock";

pub struct DesktopHarnessBuilder {
    window_size: Option<Size<Pixels>>,
    connector: FakeConnector,
    projects: Vec<PathBuf>,
    sessions: TestStore,
    saved_sessions: bool,
    state_dir: Option<PathBuf>,
    now: DateTime<Utc>,
}

pub struct FakeClock(pub DateTime<Utc>);

impl Clock for FakeClock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}

/// A [`Connector`] that serves each thread an in-memory [`FakeAgent`] instead of starting a
/// process, whatever agent the thread runs.
///
/// Agents are scripted per working directory, and every thread connecting in a directory takes
/// the next one scripted for it. A thread with no agent left to take fails to connect, as if its
/// agent could not be started. Like the real agent, each one also serves the git diff of its
/// directory, from a [`FakeGit`] repository that is clean unless one is given with
/// [`git`](Self::git).
///
/// An agent with a `launch` command gets an instance of its own, named `instance-<n>` in launch
/// order, which its connections need and its `terminate` command ends. Clones share their
/// scripts, repositories, connection counts and instances.
#[derive(Clone, Default)]
pub struct FakeConnector {
    state: Arc<Mutex<FakeConnectorState>>,
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
            now: parse_time(DEFAULT_NOW),
        }
    }
}

impl DesktopHarnessBuilder {
    /// Opens the window at this size instead of the size it had when the app last ran.
    pub fn window_size(mut self, width: f32, height: f32) -> Self {
        self.window_size = Some(size(px(width), px(height)));
        self
    }

    /// Runs the app as if it were `now`, an RFC 3339 time, instead of [`DEFAULT_NOW`].
    pub fn now(mut self, now: &str) -> Self {
        self.now = parse_time(now);
        self
    }

    /// Connects the app's threads through `connector`, for tests that script agents without
    /// adding their projects.
    pub fn connector(mut self, connector: FakeConnector) -> Self {
        self.connector = connector;
        self
    }

    /// Adds `cwd` as a project and scripts `agent` as the next agent a thread there talks to.
    pub fn agent(mut self, cwd: impl Into<PathBuf>, agent: FakeAgent) -> Self {
        let cwd = cwd.into();
        self.connector = self.connector.agent(&cwd, agent);
        self.project(cwd)
    }

    /// Serves `git` as the repository every agent in `cwd` diffs.
    pub fn git(mut self, cwd: impl Into<PathBuf>, git: FakeGit) -> Self {
        self.connector = self.connector.git(cwd, git);
        self
    }

    /// Adds `root` as a project before the app opens, as if through "Add project".
    pub fn project(mut self, root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        if !self.projects.contains(&root) {
            self.projects.push(root);
        }
        self
    }

    /// Saves a session that ran in `cwd` at `created_at`, titled by its first `prompt` if it has
    /// one. [`build`](Self::build) then waits for the app to read history.
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

    /// Keeps the app's state in [`STATE_FILE`] inside `dir`, as if it were the app's state
    /// directory, so that a harness built later over the same `dir` sees what this one saved.
    /// Without one the app remembers nothing between harnesses.
    pub fn state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
    }

    pub fn build(self, cx: &mut TestAppContext) -> DesktopHarness {
        // Single-threaded and driven only by `run_until`, so agent connections and the gpui test
        // scheduler take turns on the test thread instead of racing each other.
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
        let config = DesktopConfig {
            connector: Arc::new(self.connector.clone()),
            clock: Arc::new(FakeClock(self.now)),
            agents: Agents::new(
                AgentCommand::parse(DEFAULT_AGENT_COMMAND).expect("the default agent command parses"),
                None,
            ),
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
        };
        // The app activates itself once its window opens, which brings the window to the front.
        harness.activate(cx);
        if self.saved_sessions {
            let history = harness.history(cx);
            harness.run_until(cx, |cx| !history.read(cx).projects().is_empty());
        }
        harness
    }
}

/// A rendered [`DesktopApp`] driven through gpui's test platform. It dereferences to the
/// [`WindowDriver`] that clicks, types and measures in its window.
pub struct DesktopHarness {
    window: WindowHandle<DesktopApp>,
    driver: WindowDriver,
    runtime: Runtime,
    connector: FakeConnector,
    sessions: TestStore,
}

impl FakeConnector {
    /// Scripts `agent` as the next agent a thread connecting in `cwd` talks to.
    pub fn agent(self, cwd: impl Into<PathBuf>, agent: FakeAgent) -> Self {
        self.lock().scripts.entry(cwd.into()).or_default().push_back(agent);
        self
    }

    /// Serves `git` as the repository every agent in `cwd` diffs. The test keeps a clone to
    /// change the repository or read what the app did to it.
    pub fn git(self, cwd: impl Into<PathBuf>, git: FakeGit) -> Self {
        self.lock().gits.insert(cwd.into(), git);
        self
    }

    /// How many agents served in `cwd` still have a client connected.
    pub fn live_agents(&self, cwd: impl AsRef<Path>) -> usize {
        self.lock().live.get(cwd.as_ref()).map_or(0, Vec::len)
    }

    /// The instances launched and not yet terminated, in launch order.
    pub fn instances(&self) -> Vec<InstanceId> {
        self.lock().instances.clone()
    }

    /// How many instances were launched, terminated or not.
    pub fn launches(&self) -> usize {
        self.lock().launches
    }

    /// Ends every agent served in `cwd`, as if its process died, which closes its connection.
    pub fn kill_agents(&self, cwd: impl AsRef<Path>) {
        let agents = self.lock().live.remove(cwd.as_ref()).unwrap_or_default();
        for agent in agents {
            agent.task.abort();
        }
    }

    /// Runs `agent` over `transport` until the client's side of the connection is dropped or the
    /// agent is killed.
    fn serve(&self, cwd: &Path, agent: FakeAgent, transport: Channel) {
        let served = self.clone();
        let served_cwd = cwd.to_path_buf();
        // Held while spawning so the agent cannot end before it is counted.
        let mut state = self.lock();
        let id = state.next_agent;
        state.next_agent += 1;
        let git = agent
            .aether_capabilities()
            .git_diff
            .then(|| state.gits.get(cwd).cloned().unwrap_or_else(|| FakeGit::new(cwd)));
        let task = tokio::spawn(async move {
            let _ = match git {
                Some(git) => agent.agent().with_handler(FakeGitDiffServer::new(git)).connect_to(transport).await,
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

impl Connector for FakeConnector {
    fn launch<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, Option<InstanceId>> {
        Box::pin(async move {
            if target.agent.config.launch().is_none() {
                return Ok(None);
            }
            let mut state = self.lock();
            let instance = InstanceId::new(format!("instance-{}", state.launches));
            state.launches += 1;
            state.instances.push(instance.clone());
            Ok(Some(instance))
        })
    }

    fn connect<'a>(&'a self, target: &'a SessionTarget, mode: ConnectMode) -> ConnectorFuture<'a, Session> {
        Box::pin(async move {
            let SessionTarget { cwd, agent, instance } = target;
            let missing = |reason: &'static str| ConnectError::Spawn {
                program: agent.name.clone(),
                source: io::Error::new(io::ErrorKind::NotFound, reason),
            };
            let launched = instance.as_ref().is_some_and(|instance| self.lock().instances.contains(instance));
            if agent.config.launch().is_some() && !launched {
                return Err(missing("the agent's instance is not running"));
            }
            let script =
                self.lock().take(cwd).ok_or_else(|| missing("no fake agent is scripted for this directory"))?;
            let (agent_transport, client_transport) = Channel::duplex();
            self.serve(cwd, script, agent_transport);
            let connection = Connection::connect_to(client_info(), client_transport, cwd.clone()).await?;
            Ok(mode.open(connection).await?)
        })
    }

    fn diff_endpoint<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, Option<Endpoint>> {
        Box::pin(async move {
            match &target.agent.config.diff {
                None => Ok(None),
                Some(EndpointSource::Fixed(endpoint)) => Ok(Some(endpoint.expand(&target.vars())?)),
                Some(EndpointSource::Command(_)) => std::future::pending().await,
            }
        })
    }

    fn terminate<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, ()> {
        Box::pin(async move {
            if let Some(instance) = target.instance.as_ref().filter(|_| target.agent.config.terminate().is_some()) {
                self.lock().instances.retain(|live| live != instance);
            }
            Ok(())
        })
    }
}

impl Deref for DesktopHarness {
    type Target = WindowDriver;

    fn deref(&self) -> &WindowDriver {
        &self.driver
    }
}

impl DesktopHarness {
    pub fn threads(&self, cx: &TestAppContext) -> Entity<ThreadManager> {
        cx.read(|cx| self.app(cx).threads().clone())
    }

    pub fn selected_thread(&self, cx: &TestAppContext) -> Option<Entity<Thread>> {
        cx.read(|cx| self.app(cx).threads().read(cx).selected().cloned())
    }

    /// Starts a thread in `cwd` with its project's default agent, and selects it.
    pub fn new_thread(&self, cx: &mut TestAppContext, cwd: impl Into<PathBuf>) -> Entity<Thread> {
        let cwd = cwd.into();
        let (threads, agents) = cx.read(|cx| (self.app(cx).threads().clone(), self.app(cx).agents().clone()));
        let agent = cx.read(|cx| agents.read(cx).catalog(&git_root(&cwd)).default_agent().clone());
        threads.update(cx, |threads, cx| threads.new_thread(cwd, agent, cx))
    }

    /// Starts a thread in `cwd` like [`new_thread`](Self::new_thread) and waits for its agent to
    /// connect.
    pub fn start_thread(&self, cx: &mut TestAppContext, cwd: impl Into<PathBuf>) -> Entity<Thread> {
        let thread = self.new_thread(cx, cwd);
        self.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
        thread
    }

    /// Types `prompt` into the selected thread's composer, sends it, and waits for the agent's
    /// turn on it to end.
    pub fn send(&self, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
        self.until_answered(cx, thread, prompt, |harness, cx| {
            harness.simulate_input(cx, prompt);
            harness.simulate_keystrokes(cx, "enter");
        });
    }

    /// Submits `prompt` to `thread`, selected or not, and waits for the agent's turn on it to end.
    pub fn submit(&self, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
        self.until_answered(cx, thread, prompt, |_, cx| thread.update(cx, |thread, cx| thread.submit(prompt, cx)));
    }

    pub fn agents(&self, cx: &TestAppContext) -> Entity<Agents> {
        cx.read(|cx| self.app(cx).agents().clone())
    }

    pub fn history(&self, cx: &TestAppContext) -> Entity<History> {
        cx.read(|cx| self.app(cx).history().clone())
    }

    pub fn registry(&self, cx: &TestAppContext) -> Entity<Registry> {
        cx.read(|cx| self.app(cx).registry().clone())
    }

    /// The connector the app's threads reach their agents through. Clones share its scripts.
    pub fn connector(&self) -> &FakeConnector {
        &self.connector
    }

    /// Saves a session in the store the app reads history from, as an agent would.
    pub fn save_session(&self, session_id: &str, cwd: &str, created_at: &str, prompt: Option<&str>) {
        save_session(&self.sessions, session_id, cwd, created_at, prompt);
    }

    /// The title the sidebar lists for the row tagged `selector`, if it lists that row.
    pub fn row_title(&self, cx: &TestAppContext, selector: &str) -> Option<String> {
        cx.read(|cx| self.app(cx).sidebar().read(cx).row_title(selector))
    }

    pub fn header_title(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| {
            let app = self.app(cx);
            let selected = app.threads().read(cx).selected()?;
            Some(app.thread_view(selected)?.read(cx).title(cx))
        })
    }

    pub fn composer_text(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| {
            let app = self.app(cx);
            let selected = app.threads().read(cx).selected()?;
            let view = app.thread_view(selected)?.read(cx);
            Some(view.composer().read(cx).text(cx).to_string())
        })
    }

    pub fn changes_label(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| {
            let app = self.app(cx);
            let selected = app.threads().read(cx).selected()?;
            Some(app.thread_view(selected)?.read(cx).changes_label())
        })
    }

    /// Lets gpui and the agent connections take turns until `condition` holds.
    ///
    /// It waits on no clock: a condition that never comes true, because of a bug, makes the test
    /// hang rather than pass.
    pub fn run_until(&self, cx: &mut TestAppContext, condition: impl Fn(&App) -> bool) {
        loop {
            cx.run_until_parked();
            if cx.read(&condition) {
                return;
            }
            // Runs every Tokio task that is ready, then returns.
            self.runtime.block_on(yield_now());
        }
    }

    /// Lets gpui and the agent connections take turns until the element tagged `selector` is
    /// painted. Like [`run_until`](Self::run_until), it waits on no clock.
    pub fn run_until_painted(&self, cx: &mut TestAppContext, selector: &'static str) {
        while self.bounds(cx, selector).is_none() {
            self.runtime.block_on(yield_now());
        }
    }

    /// Brings the window to the front, as if the user switched back to the app.
    pub fn activate(&self, cx: &mut TestAppContext) {
        cx.update_window(*self.window, |_, window, _| window.activate_window()).expect("the test window is open");
        self.draw(cx);
    }

    /// Sends the window to the back, as if the user switched to another app.
    pub fn deactivate(&self, cx: &mut TestAppContext) {
        VisualTestContext::from_window(*self.window, cx).deactivate_window();
        self.draw(cx);
    }

    pub fn is_active(&self, cx: &mut TestAppContext) -> bool {
        cx.update_window(*self.window, |_, window, _| window.is_window_active()).expect("the test window is open")
    }

    /// Clicks the body of the notification tagged `tag`, as the user would in the system's
    /// notification center.
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

    /// Runs `send` and waits until `thread` is idle with `prompt` once more in its conversation.
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

#[derive(Default)]
struct FakeConnectorState {
    scripts: HashMap<PathBuf, VecDeque<FakeAgent>>,
    gits: HashMap<PathBuf, FakeGit>,
    /// The agents serving a client in each directory.
    live: HashMap<PathBuf, Vec<ServedAgent>>,
    next_agent: u64,
    /// The instances launched and not yet terminated.
    instances: Vec<InstanceId>,
    launches: usize,
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

fn parse_time(time: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(time).expect("an RFC 3339 time").to_utc()
}
