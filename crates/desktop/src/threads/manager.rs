use super::thread::{Thread, ThreadEvent};
use crate::agents::{AgentProfile, Catalogs};
use crate::clock::Clock;
use crate::history::git_root;
use crate::remotes::RemoteSpec;
use crate::target::{AgentConnector, SessionTarget};
use agent_client_protocol::v2::schema::SessionId;
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Scope, Signal, Stored, signal, stored};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Copy)]
pub struct ThreadManager {
    state: Signal<Manager>,
    connector: Stored<Arc<AgentConnector>>,
    agents: Catalogs,
    clock: Stored<Arc<dyn Clock>>,
    attention: Stored<Callback<(Attention,)>>,
    scope: Scope,
}

#[derive(Default)]
struct Manager {
    threads: Vec<Thread>,
    selected: Option<Thread>,
    unread: HashSet<usize>,
    window_active: bool,
    opened: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ThreadRef {
    Live(Thread),
    Saved { session_id: SessionId, cwd: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Attention {
    pub thread: Thread,
    pub event: ThreadEvent,
    pub watched: bool,
}

impl ThreadRef {
    pub fn live(&self) -> Option<Thread> {
        match self {
            Self::Live(thread) => Some(*thread),
            Self::Saved { .. } => None,
        }
    }
}

impl ThreadManager {
    pub fn new(connector: Arc<AgentConnector>, agents: Catalogs, clock: Arc<dyn Clock>) -> Self {
        Self {
            state: signal(Manager::default()),
            connector: stored(connector),
            agents,
            clock: stored(clock),
            attention: stored(Callback::default()),
            scope: Scope::new(),
        }
    }

    pub fn on_attention(&self, listener: impl Fn(Attention) + 'static) {
        self.attention.set(Callback::from(listener));
    }

    pub fn threads(&self) -> Vec<Thread> {
        self.state.with(|manager| manager.threads.clone())
    }

    pub fn projects(&self) -> Vec<PathBuf> {
        self.threads().iter().filter_map(Thread::project).collect()
    }

    pub fn thread_with_session(&self, session_id: &SessionId) -> Option<Thread> {
        self.threads().into_iter().find(|thread| thread.session_id().as_ref() == Some(session_id))
    }

    pub fn selected(&self) -> Option<Thread> {
        self.state.with(|manager| manager.selected)
    }

    pub fn select(&self, thread: Thread) {
        self.state.update(|manager| {
            if manager.threads.contains(&thread) {
                manager.selected = Some(thread);
                manager.unread.remove(&thread.number());
            }
        });
    }

    pub fn set_window_active(&self, active: bool) {
        self.state.update(|manager| {
            manager.window_active = active;
            if let Some(selected) = manager.selected.filter(|_| active) {
                manager.unread.remove(&selected.number());
            }
        });
    }

    pub fn is_watched(&self, thread: Thread) -> bool {
        self.state.with(|manager| manager.window_active && manager.selected == Some(thread))
    }

    pub fn is_unread(&self, thread: Thread) -> bool {
        self.state.with(|manager| manager.unread.contains(&thread.number()))
    }

    pub fn new_thread(&self, cwd: PathBuf, agent: AgentProfile) -> Thread {
        self.spawn_thread(SessionTarget::agent(cwd, agent), None)
    }

    pub fn new_cloud_thread(&self, remote: &RemoteSpec) -> Thread {
        self.spawn_thread(SessionTarget::remote(remote, None), None)
    }

    pub fn attach(&self, remote: &RemoteSpec, instance: &str) -> Thread {
        let attached = self.threads().into_iter().find(|thread| {
            let target = thread.target();
            target.remote_name() == Some(remote.name.as_str()) && target.instance() == Some(instance)
        });
        match attached {
            Some(thread) => self.reconnect_and_select(thread),
            None => self.spawn_thread(SessionTarget::remote(remote, Some(instance.to_owned())), None),
        }
    }

    pub fn open(&self, thread: &ThreadRef) {
        match thread {
            ThreadRef::Live(thread) => self.select(*thread),
            ThreadRef::Saved { session_id, cwd } => {
                self.resume(session_id.clone(), cwd.clone());
            }
        }
    }

    pub fn resume(&self, session_id: SessionId, cwd: PathBuf) -> Thread {
        if let Some(thread) = self.thread_with_session(&session_id) {
            return self.reconnect_and_select(thread);
        }
        let agent = self.default_agent(&cwd);
        self.spawn_thread(SessionTarget::agent(cwd, agent), Some(session_id))
    }

    pub fn default_agent(&self, cwd: &Path) -> AgentProfile {
        self.agents.catalog(&git_root(cwd)).default_agent().clone()
    }

    pub fn stop(&self, thread: Thread) {
        let removed = self.state.try_update(|manager| {
            let index = manager.threads.iter().position(|listed| *listed == thread)?;
            manager.threads.remove(index);
            manager.unread.remove(&thread.number());
            if manager.selected == Some(thread) {
                manager.selected = None;
            }
            Some(())
        });
        if removed.flatten().is_some() {
            thread.stop();
        }
    }

    fn reconnect_and_select(&self, thread: Thread) -> Thread {
        thread.reconnect();
        self.select(thread);
        thread
    }

    fn spawn_thread(&self, target: SessionTarget, session_id: Option<SessionId>) -> Thread {
        let number = self
            .state
            .try_update(|manager| {
                manager.opened += 1;
                manager.opened - 1
            })
            .unwrap_or_default();
        let (connector, opened_at) = (self.connector.get(), self.clock.with(|clock| clock.now()));
        let manager = *self;
        let thread = self.scope.run(|| {
            let thread = Thread::connect(number, target, session_id, connector, opened_at);
            thread.on_event(move |event| manager.on_thread_event(thread, event));
            thread
        });
        self.state.update(|manager| {
            manager.threads.push(thread);
            manager.selected = Some(thread);
        });
        thread
    }

    fn on_thread_event(&self, thread: Thread, event: ThreadEvent) {
        if !matches!(event, ThreadEvent::TurnFinished | ThreadEvent::NeedsInput) {
            return;
        }
        let watched = self.state.with_untracked(|manager| manager.window_active && manager.selected == Some(thread));
        if !watched && event == ThreadEvent::TurnFinished {
            self.state.update(|manager| {
                manager.unread.insert(thread.number());
            });
        }
        self.attention.get().call(Attention { thread, event, watched });
    }
}
