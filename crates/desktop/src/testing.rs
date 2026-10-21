//! Integration-test support: opens the production [`DesktopApp`] in a gpui test window, with
//! in-memory agents in place of agent processes.

use crate::history::History;
use crate::registry::{Registry, STATE_FILE};
use crate::target::{
    AgentCommandLine, ConnectError, ConnectFuture, ConnectMode, Connector, SessionTarget, client_info,
};
use crate::threads::{Thread, ThreadManager};
use crate::{DesktopApp, DesktopConfig, DesktopTheme, cli::DEFAULT_AGENT, init, runtime, window_options, window_size};
use acp_utils::testing::{Channel, FakeAgent};
use aether_sessions::SessionStore;
use aether_sessions::testing::TestStore;
use client_core::session::Connection;
use client_core::testing::{FakeGit, FakeGitDiffServer};
use gpui::{
    App, AppContext, Bounds, Entity, Modifiers, MouseButton, Pixels, ScrollDelta, ScrollWheelEvent, Size,
    SystemNotificationResponse, TestAppContext, VisualTestContext, WindowHandle, point, px, size,
};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::runtime::{Builder, Runtime};
use tokio::task::{AbortHandle, yield_now};

/// The file the `fake-acp-agent` test binary holds an exclusive lock on, in its working
/// directory, for as long as it runs.
pub const FAKE_AGENT_LOCK_FILE: &str = "agent.lock";

/// Configures and opens a [`DesktopHarness`].
pub struct DesktopHarnessBuilder {
    window_size: Option<Size<Pixels>>,
    connector: FakeConnector,
    projects: Vec<PathBuf>,
    sessions: TestStore,
    state_dir: Option<PathBuf>,
}

/// A [`Connector`] that serves each thread an in-memory [`FakeAgent`] instead of starting a
/// process.
///
/// Agents are scripted per working directory, and every thread connecting in a directory takes
/// the next one scripted for it. A thread with no agent left to take fails to connect, as if its
/// agent could not be started. Like the real agent, each one also serves the git diff of its
/// directory, from a [`FakeGit`] repository that is clean unless one is given with
/// [`git`](Self::git). Clones share their scripts, repositories and connection counts.
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
            state_dir: None,
        }
    }
}

impl DesktopHarnessBuilder {
    /// Opens the window at this size instead of the size it had when the app last ran.
    pub fn window_size(mut self, width: f32, height: f32) -> Self {
        self.window_size = Some(size(px(width), px(height)));
        self
    }

    /// Connects the app's threads through `connector`.
    pub fn connector(mut self, connector: FakeConnector) -> Self {
        self.connector = connector;
        self
    }

    /// Adds `root` as a project before the app opens, as if through "Add project".
    pub fn project(mut self, root: impl Into<PathBuf>) -> Self {
        self.projects.push(root.into());
        self
    }

    /// Keeps the app's state in [`STATE_FILE`] inside `dir`, as if it were the app's state
    /// directory, so that a harness built later over the same `dir` sees what this one saved.
    /// Without one the app remembers nothing between harnesses.
    pub fn state_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.state_dir = Some(dir.into());
        self
    }

    /// Reads history from `sessions` instead of an empty store. The harness keeps the store, so a
    /// test can write more sessions to it through [`DesktopHarness::sessions`].
    pub fn history(mut self, sessions: TestStore) -> Self {
        self.sessions = sessions;
        self
    }

    /// Opens the configured app and settles the first frame.
    ///
    /// # Panics
    ///
    /// Panics if gpui cannot open or draw the test window.
    pub fn build(self, cx: &mut TestAppContext) -> DesktopHarness {
        // Single-threaded and driven only by `run_until`, so agent connections and the gpui test
        // scheduler take turns on the test thread instead of racing each other.
        let runtime = Builder::new_current_thread().enable_all().build().expect("start the test Tokio runtime");
        cx.update(|cx| {
            runtime::install(cx, runtime.handle().clone());
            init(cx);
        });
        let registry = cx.new(|cx| {
            let mut registry = Registry::load(self.state_dir.map(|dir| dir.join(STATE_FILE)), cx);
            for root in self.projects {
                registry.add_project(root, cx);
            }
            registry
        });
        let window_size = self.window_size.unwrap_or_else(|| cx.read(|cx| window_size(&registry, cx)));
        let bounds = Bounds::new(point(px(0.0), px(0.0)), window_size);
        let config = DesktopConfig {
            connector: Arc::new(self.connector),
            agent: AgentCommandLine::new(DEFAULT_AGENT),
            sessions: Some(SessionStore::from_path(self.sessions.path().to_path_buf())),
            registry,
        };
        let window = cx.update(|cx| {
            cx.open_window(window_options(bounds), |window, cx| cx.new(|cx| DesktopApp::new(config, window, cx)))
                .expect("open the gpui test window")
        });
        let harness = DesktopHarness { window, runtime, sessions: self.sessions };
        // The app activates itself once its window opens, which brings the window to the front.
        harness.activate(cx);
        harness
    }
}

/// A rendered [`DesktopApp`] driven through gpui's test platform.
pub struct DesktopHarness {
    window: WindowHandle<DesktopApp>,
    runtime: Runtime,
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
        let git = state.gits.get(cwd).cloned().unwrap_or_else(|| FakeGit::new(cwd));
        let task = tokio::spawn(async move {
            let _ = agent.agent().with_handler(FakeGitDiffServer::new(git)).connect_to(transport).await;
            served.lock().disconnected(&served_cwd, id);
        });
        state.live.entry(cwd.to_path_buf()).or_default().push(ServedAgent { id, task: task.abort_handle() });
    }

    fn lock(&self) -> MutexGuard<'_, FakeConnectorState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Connector for FakeConnector {
    fn connect<'a>(&'a self, target: &'a SessionTarget, mode: ConnectMode) -> ConnectFuture<'a> {
        Box::pin(async move {
            let SessionTarget::Local { cwd, agent: command } = target;
            let agent = self.lock().take(cwd).ok_or_else(|| ConnectError::Spawn {
                command: command.clone(),
                source: io::Error::new(io::ErrorKind::NotFound, "no fake agent is scripted for this directory"),
            })?;
            let (agent_transport, client_transport) = Channel::duplex();
            self.serve(cwd, agent, agent_transport);
            let connection = Connection::connect_to(client_info(), client_transport, cwd.clone()).await?;
            Ok(mode.open(connection).await?)
        })
    }
}

impl DesktopHarness {
    /// The app's threads.
    pub fn threads(&self, cx: &TestAppContext) -> Entity<ThreadManager> {
        cx.read(|cx| self.app(cx).threads().clone())
    }

    /// The selected thread, if any.
    pub fn selected_thread(&self, cx: &TestAppContext) -> Option<Entity<Thread>> {
        cx.read(|cx| {
            let threads = self.app(cx).threads().read(cx);
            threads.selected().and_then(|id| threads.thread(id, cx)).cloned()
        })
    }

    pub fn history(&self, cx: &TestAppContext) -> Entity<History> {
        cx.read(|cx| self.app(cx).history().clone())
    }

    pub fn registry(&self, cx: &TestAppContext) -> Entity<Registry> {
        cx.read(|cx| self.app(cx).registry().clone())
    }

    /// The title the sidebar lists for the row tagged `selector`, if it lists that row.
    pub fn row_title(&self, cx: &TestAppContext, selector: &str) -> Option<String> {
        cx.read(|cx| self.app(cx).sidebar().read(cx).row_title(selector, cx))
    }

    /// The title in the selected thread's header, if a thread is selected.
    pub fn header_title(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| {
            let app = self.app(cx);
            let selected = app.threads().read(cx).selected()?;
            Some(app.thread_view(selected)?.read(cx).title(cx))
        })
    }

    /// The store the app reads history from.
    pub fn sessions(&self) -> &TestStore {
        &self.sessions
    }

    /// The text in the selected thread's composer, if a thread is selected.
    pub fn composer_text(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| {
            let app = self.app(cx);
            let selected = app.threads().read(cx).selected()?;
            let view = app.thread_view(selected)?.read(cx);
            Some(view.composer().read(cx).text(cx).to_string())
        })
    }

    /// The label of the selected thread's Changes tab, if a thread is selected.
    pub fn changes_label(&self, cx: &TestAppContext) -> Option<String> {
        cx.read(|cx| {
            let app = self.app(cx);
            let selected = app.threads().read(cx).selected()?;
            Some(app.thread_view(selected)?.read(cx).changes_label(cx))
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

    /// Draws and settles the test window.
    ///
    /// # Panics
    ///
    /// Panics if gpui cannot draw the test window.
    pub fn draw(&self, cx: &mut TestAppContext) {
        cx.update_window(*self.window, |_, window, cx| window.draw(cx).clear(cx)).expect("draw the gpui test window");
        cx.run_until_parked();
    }

    pub fn simulate_keystrokes(&self, cx: &mut TestAppContext, keystrokes: &str) {
        cx.simulate_keystrokes(*self.window, keystrokes);
        self.draw(cx);
    }

    pub fn simulate_input(&self, cx: &mut TestAppContext, text: &str) {
        cx.simulate_input(*self.window, text);
        self.draw(cx);
    }

    /// Clicks the centre of the element tagged with `selector`.
    pub fn click(&self, cx: &mut TestAppContext, selector: &'static str) -> Result<(), HarnessError> {
        let bounds = self.bounds(cx, selector).ok_or(HarnessError::MissingElement(selector))?;
        VisualTestContext::from_window(*self.window, cx).simulate_click(bounds.center(), Modifiers::default());
        self.draw(cx);
        Ok(())
    }

    /// Right-clicks the centre of the element tagged with `selector`.
    pub fn right_click(&self, cx: &mut TestAppContext, selector: &'static str) -> Result<(), HarnessError> {
        let center = self.bounds(cx, selector).ok_or(HarnessError::MissingElement(selector))?.center();
        let mut window = VisualTestContext::from_window(*self.window, cx);
        window.simulate_mouse_down(center, MouseButton::Right, Modifiers::default());
        window.simulate_mouse_up(center, MouseButton::Right, Modifiers::default());
        self.draw(cx);
        // A menu paints a frame after the frame that opens it, once it knows where its row is.
        self.draw(cx);
        Ok(())
    }

    /// Brings the window to the front, as if the user switched back to the app.
    ///
    /// # Panics
    ///
    /// Panics if the test window has closed.
    pub fn activate(&self, cx: &mut TestAppContext) {
        cx.update_window(*self.window, |_, window, _| window.activate_window()).expect("the test window is open");
        self.draw(cx);
    }

    /// Sends the window to the back, as if the user switched to another app.
    pub fn deactivate(&self, cx: &mut TestAppContext) {
        VisualTestContext::from_window(*self.window, cx).deactivate_window();
        self.draw(cx);
    }

    /// Whether the window is in front.
    ///
    /// # Panics
    ///
    /// Panics if the test window has closed.
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

    /// Scrolls the element tagged `selector` by `lines` lines, upwards for positive `lines`, as
    /// if with the mouse wheel over its centre.
    pub fn scroll(&self, cx: &mut TestAppContext, selector: &'static str, lines: f32) -> Result<(), HarnessError> {
        let position = self.bounds(cx, selector).ok_or(HarnessError::MissingElement(selector))?.center();
        VisualTestContext::from_window(*self.window, cx).simulate_event(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Lines(point(0.0, lines)),
            ..ScrollWheelEvent::default()
        });
        self.draw(cx);
        Ok(())
    }

    /// Resizes the window, as if the user dragged its edge.
    pub fn resize(&self, cx: &mut TestAppContext, width: f32, height: f32) {
        cx.simulate_window_resize(self.window.into(), size(px(width), px(height)));
        self.draw(cx);
    }

    /// Returns the painted bounds of the element tagged with `selector`.
    pub fn bounds(&self, cx: &mut TestAppContext, selector: &'static str) -> Option<Bounds<Pixels>> {
        self.draw(cx);
        VisualTestContext::from_window(*self.window, cx).debug_bounds(selector)
    }

    /// Reads the installed app theme.
    pub fn theme(&self, cx: &TestAppContext) -> DesktopTheme {
        cx.read(|cx| DesktopTheme::global(cx).clone())
    }

    fn app<'a>(&self, cx: &'a App) -> &'a DesktopApp {
        self.window.read(cx).expect("the test window is open")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("no painted element is tagged `{0}`")]
    MissingElement(&'static str),
}

#[derive(Default)]
struct FakeConnectorState {
    scripts: HashMap<PathBuf, VecDeque<FakeAgent>>,
    gits: HashMap<PathBuf, FakeGit>,
    /// The agents serving a client in each directory.
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
