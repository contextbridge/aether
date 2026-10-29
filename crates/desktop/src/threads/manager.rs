use super::observe_thread;
use super::thread::{Thread, ThreadEvent};
use crate::agents::{AgentProfile, Agents};
use crate::clock::Clock;
use crate::history::git_root;
use crate::remotes::RemoteSpec;
use crate::target::{AgentConnector, SessionTarget};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{App, AppContext, Context, Entity, EntityId, EventEmitter, Subscription};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct ThreadManager {
    connector: Arc<AgentConnector>,
    agents: Entity<Agents>,
    clock: Arc<dyn Clock>,
    threads: Vec<Entity<Thread>>,
    listings: HashMap<EntityId, Subscription>,
    selected: Option<Entity<Thread>>,
    unread: HashSet<EntityId>,
    window_active: bool,
    opened: usize,
}

#[derive(Clone, PartialEq)]
pub enum ThreadRef {
    Live(Entity<Thread>),
    Saved { session_id: SessionId, cwd: PathBuf },
}

#[derive(Debug)]
pub struct Attention {
    pub thread: Entity<Thread>,
    pub event: ThreadEvent,
    pub watched: bool,
}

impl ThreadManager {
    pub fn new(connector: Arc<AgentConnector>, agents: Entity<Agents>, clock: Arc<dyn Clock>) -> Self {
        Self {
            connector,
            agents,
            clock,
            threads: Vec::new(),
            listings: HashMap::new(),
            selected: None,
            unread: HashSet::new(),
            window_active: false,
            opened: 0,
        }
    }

    pub fn threads(&self) -> &[Entity<Thread>] {
        &self.threads
    }

    pub fn projects<'a>(&'a self, cx: &'a App) -> impl Iterator<Item = &'a Path> {
        self.threads.iter().filter_map(|thread| thread.read(cx).project())
    }

    pub fn thread_with_session(&self, session_id: &SessionId, cx: &App) -> Option<&Entity<Thread>> {
        self.threads.iter().find(|thread| thread.read(cx).session_id() == Some(session_id))
    }

    pub fn selected(&self) -> Option<&Entity<Thread>> {
        self.selected.as_ref()
    }

    pub fn select(&mut self, thread: &Entity<Thread>, cx: &mut Context<Self>) {
        if self.threads.contains(thread) {
            self.selected = Some(thread.clone());
            self.unread.remove(&thread.entity_id());
            cx.notify();
        }
    }

    pub fn set_window_active(&mut self, active: bool, cx: &mut Context<Self>) {
        self.window_active = active;
        if let Some(selected) = self.selected.as_ref().filter(|_| active) {
            self.unread.remove(&selected.entity_id());
        }
        cx.notify();
    }

    pub fn is_watched(&self, thread: &Entity<Thread>) -> bool {
        self.window_active && self.selected.as_ref() == Some(thread)
    }

    pub fn is_unread(&self, thread: &Entity<Thread>) -> bool {
        self.unread.contains(&thread.entity_id())
    }

    pub fn new_thread(&mut self, cwd: PathBuf, agent: AgentProfile, cx: &mut Context<Self>) -> Entity<Thread> {
        self.spawn_thread(SessionTarget::agent(cwd, agent), None, cx)
    }

    pub fn new_cloud_thread(&mut self, remote: &RemoteSpec, cx: &mut Context<Self>) -> Entity<Thread> {
        self.spawn_thread(SessionTarget::remote(remote, None), None, cx)
    }

    pub fn attach(&mut self, remote: &RemoteSpec, instance: &str, cx: &mut Context<Self>) -> Entity<Thread> {
        let attached = self.threads.iter().find(|thread| {
            let target = thread.read(cx).target();
            target.remote_name() == Some(remote.name.as_str()) && target.instance() == Some(instance)
        });
        match attached.cloned() {
            Some(thread) => self.reconnect_and_select(thread, cx),
            None => self.spawn_thread(SessionTarget::remote(remote, Some(instance.to_owned())), None, cx),
        }
    }

    pub fn open(&mut self, thread: &ThreadRef, cx: &mut Context<Self>) {
        match thread {
            ThreadRef::Live(thread) => self.select(thread, cx),
            ThreadRef::Saved { session_id, cwd } => drop(self.resume(session_id.clone(), cwd.clone(), cx)),
        }
    }

    pub fn resume(&mut self, session_id: SessionId, cwd: PathBuf, cx: &mut Context<Self>) -> Entity<Thread> {
        if let Some(thread) = self.thread_with_session(&session_id, cx).cloned() {
            return self.reconnect_and_select(thread, cx);
        }
        let agent = self.default_agent(&cwd, cx);
        self.spawn_thread(SessionTarget::agent(cwd, agent), Some(session_id), cx)
    }

    pub fn default_agent(&self, cwd: &Path, cx: &mut App) -> AgentProfile {
        self.agents.update(cx, |agents, _| agents.catalog(&git_root(cwd)).default_agent().clone())
    }

    pub fn stop(&mut self, thread: &Entity<Thread>, cx: &mut Context<Self>) {
        let Some(index) = self.threads.iter().position(|listed| listed == thread) else {
            return;
        };
        self.threads.remove(index).update(cx, Thread::stop);
        self.listings.remove(&thread.entity_id());
        self.unread.remove(&thread.entity_id());
        if self.selected.as_ref() == Some(thread) {
            self.selected = None;
        }
        cx.notify();
    }

    fn reconnect_and_select(&mut self, thread: Entity<Thread>, cx: &mut Context<Self>) -> Entity<Thread> {
        thread.update(cx, Thread::reconnect);
        self.select(&thread, cx);
        thread
    }

    fn spawn_thread(
        &mut self,
        target: SessionTarget,
        session_id: Option<SessionId>,
        cx: &mut Context<Self>,
    ) -> Entity<Thread> {
        let number = self.opened;
        self.opened += 1;
        let (connector, opened_at) = (self.connector.clone(), self.clock.now());
        let thread = cx.new(|cx| Thread::connect(number, target, session_id, connector, opened_at, cx));
        cx.subscribe(&thread, |this, thread, event: &ThreadEvent, cx| this.on_thread_event(thread, event, cx)).detach();
        let listing = observe_thread(
            &thread,
            cx,
            |thread| {
                let instance = thread.target().instance().map(str::to_owned);
                (thread.session_id().cloned(), thread.title(), thread.status(), instance)
            },
            |_, cx| cx.notify(),
        );
        self.listings.insert(thread.entity_id(), listing);
        self.threads.push(thread.clone());
        self.selected = Some(thread.clone());
        cx.notify();
        thread
    }

    fn on_thread_event(&mut self, thread: Entity<Thread>, event: &ThreadEvent, cx: &mut Context<Self>) {
        if !self.threads.contains(&thread) || !matches!(event, ThreadEvent::TurnFinished | ThreadEvent::NeedsInput) {
            return;
        }
        let watched = self.is_watched(&thread);
        if !watched && *event == ThreadEvent::TurnFinished {
            self.unread.insert(thread.entity_id());
            cx.notify();
        }
        cx.emit(Attention { thread, event: event.clone(), watched });
    }
}

impl EventEmitter<Attention> for ThreadManager {}
