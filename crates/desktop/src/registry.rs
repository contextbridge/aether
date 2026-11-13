use agent_client_protocol::v2::schema::SessionId;
use clankerdiff_rouge_shell::persist::Store;
use clankerdiff_rouge_shell::rouge::reactive::{Signal, Stored, signal, stored};
use clankerdiff_ui_core::ThemePreference;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

pub const STATE_FILE: &str = "state.json";

pub type Saved = Store<State>;

#[derive(Clone, Copy)]
pub struct Registry {
    state: Signal<State>,
    saved: Stored<Saved>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub projects: Vec<PathBuf>,
    pub collapsed: HashSet<Collapsed>,
    pub last_agents: HashMap<PathBuf, String>,
    pub archived: HashSet<SessionId>,
    pub titles: HashMap<SessionId, String>,
    pub window_size: Option<WindowSize>,
    pub last_selected: Option<SessionId>,
    pub interface_size: Option<f32>,
    pub sidebar_width: Option<f32>,
    pub sidebar_collapsed: bool,
    pub theme: ThemePreference,
    pub reduced_motion: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Collapsed {
    Project(PathBuf),
    Section(String),
    Remote(String),
}

pub fn default_state_file() -> Option<PathBuf> {
    utils::path::aether_home().map(|home| home.join("desktop").join(STATE_FILE))
}

pub fn load(file: Option<PathBuf>, runtime: &Handle) -> (Saved, JoinHandle<io::Result<()>>) {
    Store::load(file, runtime)
}

impl Registry {
    pub fn new(saved: Saved) -> Self {
        Self { state: signal(saved.state()), saved: stored(saved) }
    }

    pub fn with<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        self.state.with(read)
    }

    pub fn with_untracked<T>(&self, read: impl FnOnce(&State) -> T) -> T {
        self.state.with_untracked(read)
    }

    pub fn get(&self) -> State {
        self.state.get()
    }

    pub fn update(&self, change: impl FnOnce(&mut State)) {
        let mut next = self.state.get_untracked();
        change(&mut next);
        if self.state.set_if_changed(next.clone()) {
            self.saved.with(|saved| {
                saved.update(|state| {
                    *state = next;
                    true
                });
            });
        }
    }
}

impl State {
    pub fn add_project(&mut self, root: PathBuf) {
        if !self.projects.contains(&root) {
            self.projects.push(root);
        }
    }

    pub fn archive(&mut self, session_id: SessionId) {
        self.archived.insert(session_id);
    }

    pub fn toggle_collapsed(&mut self, group: Collapsed) {
        if !self.collapsed.remove(&group) {
            self.collapsed.insert(group);
        }
    }

    pub fn title(&self, session_id: Option<&SessionId>, derived: String) -> String {
        session_id.and_then(|id| self.titles.get(id)).cloned().unwrap_or(derived)
    }

    pub fn rename(&mut self, session_id: SessionId, title: &str) {
        match title.trim() {
            "" => self.titles.remove(&session_id),
            title => self.titles.insert(session_id, title.to_owned()),
        };
    }
}

impl WindowSize {
    #[must_use]
    pub fn at_least(self, minimum: Self) -> Self {
        Self { width: self.width.max(minimum.width), height: self.height.max(minimum.height) }
    }
}
