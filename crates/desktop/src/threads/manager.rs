use super::thread::{Thread, ThreadEvent, ThreadId};
use crate::target::{AgentCommandLine, ConnectMode, Connector, SessionTarget};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{App, AppContext, Context, Entity, EventEmitter};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Owns every live thread, which one is selected, and which ones finished a turn unseen.
///
/// A saved session is open in at most one thread. Stopping a thread removes it, and its session
/// goes back to being only in history.
pub struct ThreadManager {
    connector: Arc<dyn Connector>,
    agent: AgentCommandLine,
    threads: Vec<Entity<Thread>>,
    selected: Option<ThreadId>,
    unread: HashSet<ThreadId>,
    /// Whether the app's window is in front. While it is not, no thread is watched.
    window_active: bool,
    next_id: u64,
}

/// A thread the user is not [watching](ThreadManager::is_watched) did something they should see.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attention {
    pub thread: ThreadId,
    pub event: ThreadEvent,
}

impl ThreadManager {
    /// Connects new threads through `connector`, starting `agent` for each local one.
    pub fn new(connector: Arc<dyn Connector>, agent: AgentCommandLine) -> Self {
        Self {
            connector,
            agent,
            threads: Vec::new(),
            selected: None,
            unread: HashSet::new(),
            window_active: false,
            next_id: 0,
        }
    }

    /// The threads in the order they were opened.
    pub fn threads(&self) -> &[Entity<Thread>] {
        &self.threads
    }

    pub fn thread(&self, id: ThreadId, cx: &App) -> Option<&Entity<Thread>> {
        self.threads.iter().find(|thread| thread.read(cx).id() == id)
    }

    /// The live thread that has the session `session_id` open, if any.
    pub fn thread_with_session(&self, session_id: &SessionId, cx: &App) -> Option<&Entity<Thread>> {
        self.threads.iter().find(|thread| thread.read(cx).session_id() == Some(session_id))
    }

    pub fn selected(&self) -> Option<ThreadId> {
        self.selected
    }

    /// Selects the thread, which marks what it did as seen.
    pub fn select(&mut self, id: ThreadId, cx: &mut Context<Self>) {
        if self.thread(id, cx).is_some() {
            self.selected = Some(id);
            self.unread.remove(&id);
            cx.notify();
        }
    }

    /// Records whether the app's window is in front. Coming back to the front marks what the
    /// selected thread did meanwhile as seen.
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if let Some(id) = self.selected.filter(|_| active) {
            self.unread.remove(&id);
        }
        cx.notify();
    }

    /// Whether the user is watching the thread, so that what it does is seen as it happens: it is
    /// selected, in the window in front.
    pub fn is_watched(&self, id: ThreadId) -> bool {
        self.window_active && self.selected == Some(id)
    }

    /// Whether the thread finished a turn while it was not [watched](Self::is_watched), and has
    /// not been watched since.
    pub fn is_unread(&self, id: ThreadId) -> bool {
        self.unread.contains(&id)
    }

    /// Starts a thread with a fresh session in `cwd`, and selects it.
    pub fn new_thread(&mut self, cwd: PathBuf, cx: &mut Context<Self>) -> Entity<Thread> {
        self.open(cwd, ConnectMode::NewSession, cx)
    }

    /// Opens the saved session `session_id` in a live thread and selects it.
    ///
    /// A new thread resumes the session in `cwd`, replaying its history. When a thread already has
    /// the session open, that thread is selected instead, and reconnected if its connection ended.
    pub fn resume(&mut self, session_id: SessionId, cwd: PathBuf, cx: &mut Context<Self>) -> Entity<Thread> {
        let Some(thread) = self.thread_with_session(&session_id, cx).cloned() else {
            return self.open(cwd, ConnectMode::Resume(session_id), cx);
        };
        let id = thread.read(cx).id();
        self.reconnect(id, cx);
        self.select(id, cx);
        thread
    }

    /// Connects the thread again if its connection failed or closed, resuming its session when it
    /// has one. A thread that is connecting or connected is left alone.
    pub fn reconnect(&mut self, id: ThreadId, cx: &mut Context<Self>) {
        let connector = self.connector.clone();
        if let Some(thread) = self.thread(id, cx).cloned() {
            thread.update(cx, |thread, cx| thread.reconnect(connector, cx));
        }
    }

    /// Ends the thread's agent and removes the thread. Its session, once saved, shows in history
    /// again, from where it can be resumed.
    pub fn stop(&mut self, id: ThreadId, cx: &mut Context<Self>) {
        let Some(index) = self.threads.iter().position(|thread| thread.read(cx).id() == id) else {
            return;
        };
        // Stopped explicitly because other holders of the entity would otherwise keep its agent alive.
        self.threads.remove(index).update(cx, Thread::stop);
        self.unread.remove(&id);
        if self.selected == Some(id) {
            self.selected = None;
        }
        cx.notify();
    }

    fn open(&mut self, cwd: PathBuf, mode: ConnectMode, cx: &mut Context<Self>) -> Entity<Thread> {
        let id = ThreadId::new(self.next_id);
        self.next_id += 1;
        let target = SessionTarget::Local { cwd, agent: self.agent.clone() };
        let connector = self.connector.clone();
        let thread = cx.new(|cx| Thread::connect(id, target, mode, connector, cx));
        cx.subscribe(&thread, move |this, _, event: &ThreadEvent, cx| this.on_thread_event(id, event, cx)).detach();
        self.threads.push(thread.clone());
        self.selected = Some(id);
        cx.notify();
        thread
    }

    fn on_thread_event(&mut self, id: ThreadId, event: &ThreadEvent, cx: &mut Context<Self>) {
        if self.is_watched(id) || self.thread(id, cx).is_none() {
            return;
        }
        if *event == ThreadEvent::TurnFinished {
            self.unread.insert(id);
            cx.notify();
        }
        cx.emit(Attention { thread: id, event: event.clone() });
    }
}

impl EventEmitter<Attention> for ThreadManager {}
