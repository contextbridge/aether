use super::thread::{Thread, ThreadEvent};
use crate::agents::{AgentProfile, Agents};
use crate::history::git_root;
use crate::target::{ConnectMode, Connector, SessionTarget};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{App, AppContext, Context, Entity, EntityId, EventEmitter};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

/// Owns every live thread, which one is selected, and which ones finished a turn unseen.
///
/// A saved session is open in at most one thread. Stopping a thread removes it, and its session
/// goes back to being only in history.
pub struct ThreadManager {
    connector: Arc<dyn Connector>,
    agents: Entity<Agents>,
    threads: Vec<Entity<Thread>>,
    selected: Option<Entity<Thread>>,
    unread: HashSet<EntityId>,
    /// Whether the app's window is in front. While it is not, no thread is watched.
    window_active: bool,
    opened: usize,
}

/// A thread finished a turn or needs input.
#[derive(Debug)]
pub struct Attention {
    pub thread: Entity<Thread>,
    pub event: ThreadEvent,
    /// Whether the user was [watching](ThreadManager::is_watched) the thread, and so saw it.
    pub watched: bool,
}

impl ThreadManager {
    /// Connects threads through `connector`, resuming saved sessions with the agents `agents`
    /// offers.
    pub fn new(connector: Arc<dyn Connector>, agents: Entity<Agents>) -> Self {
        Self {
            connector,
            agents,
            threads: Vec::new(),
            selected: None,
            unread: HashSet::new(),
            window_active: false,
            opened: 0,
        }
    }

    /// The threads in the order they were opened.
    pub fn threads(&self) -> &[Entity<Thread>] {
        &self.threads
    }

    pub fn thread_with_session(&self, session_id: &SessionId, cx: &App) -> Option<&Entity<Thread>> {
        self.threads.iter().find(|thread| thread.read(cx).session_id() == Some(session_id))
    }

    pub fn selected(&self) -> Option<&Entity<Thread>> {
        self.selected.as_ref()
    }

    /// Selects the thread, which marks what it did as seen.
    pub fn select(&mut self, thread: &Entity<Thread>, cx: &mut Context<Self>) {
        if self.threads.contains(thread) {
            self.selected = Some(thread.clone());
            self.unread.remove(&thread.entity_id());
            cx.notify();
        }
    }

    /// Records whether the app's window is in front. Coming back to the front marks what the
    /// selected thread did meanwhile as seen.
    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if let Some(selected) = self.selected.as_ref().filter(|_| active) {
            self.unread.remove(&selected.entity_id());
        }
        cx.notify();
    }

    /// Whether the user is watching the thread, so that what it does is seen as it happens: it is
    /// selected, in the window in front.
    pub fn is_watched(&self, thread: &Entity<Thread>) -> bool {
        self.window_active && self.selected.as_ref() == Some(thread)
    }

    /// Whether the thread finished a turn while it was not [watched](Self::is_watched), and has
    /// not been watched since.
    pub fn is_unread(&self, thread: &Entity<Thread>) -> bool {
        self.unread.contains(&thread.entity_id())
    }

    /// Starts a thread with a fresh session of `agent` in `cwd`, and selects it.
    pub fn new_thread(&mut self, cwd: PathBuf, agent: AgentProfile, cx: &mut Context<Self>) -> Entity<Thread> {
        self.open(SessionTarget::new(cwd, agent), ConnectMode::NewSession, cx)
    }

    /// Opens the saved session `session_id` in a live thread and selects it.
    ///
    /// A new thread resumes the session in `cwd` with its project's
    /// [`DEFAULT_AGENT`](crate::agents::DEFAULT_AGENT), since saved sessions are Aether's,
    /// replaying its history. When a thread already has the session open, that thread is selected
    /// instead, and reconnected if its connection ended.
    pub fn resume(&mut self, session_id: SessionId, cwd: PathBuf, cx: &mut Context<Self>) -> Entity<Thread> {
        let Some(thread) = self.thread_with_session(&session_id, cx).cloned() else {
            let agent = self.agents.read(cx).catalog(&git_root(&cwd)).default_agent().clone();
            return self.open(SessionTarget::new(cwd, agent), ConnectMode::Resume(session_id), cx);
        };
        self.reconnect(&thread, cx);
        self.select(&thread, cx);
        thread
    }

    /// Connects the thread again if its connection failed or closed, resuming its session when it
    /// has one. A thread that is connecting or connected is left alone.
    pub fn reconnect(&mut self, thread: &Entity<Thread>, cx: &mut Context<Self>) {
        let connector = self.connector.clone();
        if self.threads.contains(thread) {
            thread.update(cx, |thread, cx| thread.reconnect(connector, cx));
        }
    }

    /// Ends the thread's agent, and any instance it was launched on, and removes the thread. Its
    /// session, once saved, shows in history again, from where it can be resumed.
    pub fn stop(&mut self, thread: &Entity<Thread>, cx: &mut Context<Self>) {
        let Some(index) = self.threads.iter().position(|listed| listed == thread) else {
            return;
        };
        // Stopped explicitly because other holders of the entity would otherwise keep its agent alive.
        let connector = self.connector.clone();
        self.threads.remove(index).update(cx, |thread, cx| thread.stop(connector, cx));
        self.unread.remove(&thread.entity_id());
        if self.selected.as_ref() == Some(thread) {
            self.selected = None;
        }
        cx.notify();
    }

    fn open(&mut self, target: SessionTarget, mode: ConnectMode, cx: &mut Context<Self>) -> Entity<Thread> {
        let number = self.opened;
        self.opened += 1;
        let connector = self.connector.clone();
        let thread = cx.new(|cx| Thread::connect(number, target, mode, connector, cx));
        cx.subscribe(&thread, |this, thread, event: &ThreadEvent, cx| this.on_thread_event(thread, event, cx)).detach();
        self.threads.push(thread.clone());
        self.selected = Some(thread.clone());
        cx.notify();
        thread
    }

    fn on_thread_event(&mut self, thread: Entity<Thread>, event: &ThreadEvent, cx: &mut Context<Self>) {
        if !self.threads.contains(&thread) {
            return;
        }
        match event {
            // Observers of the manager list its threads.
            ThreadEvent::RowChanged => cx.notify(),
            ThreadEvent::TurnFinished | ThreadEvent::NeedsInput => {
                let watched = self.is_watched(&thread);
                if !watched && *event == ThreadEvent::TurnFinished {
                    self.unread.insert(thread.entity_id());
                    cx.notify();
                }
                cx.emit(Attention { thread, event: event.clone(), watched });
            }
            _ => {}
        }
    }
}

impl EventEmitter<Attention> for ThreadManager {}
