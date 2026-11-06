use agent_client_protocol::v2::schema::SessionId;
use clankerdiff_gpui_shell::persist::Store;
use clankerdiff_ui_core::ThemePreference;
use gpui::{Pixels, Size};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub const STATE_FILE: &str = "state.json";

pub type Registry = Store<State>;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub projects: Vec<PathBuf>,
    pub collapsed: HashSet<Collapsed>,
    pub last_agents: HashMap<PathBuf, String>,
    pub archived: HashSet<SessionId>,
    pub titles: HashMap<SessionId, String>,
    pub window_size: Option<Size<Pixels>>,
    pub last_selected: Option<SessionId>,
    pub interface_size: Option<f32>,
    pub sidebar_width: Option<f32>,
    pub sidebar_collapsed: bool,
    pub theme: ThemePreference,
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
        toggle(&mut self.collapsed, group);
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

fn toggle<T: Eq + std::hash::Hash>(set: &mut HashSet<T>, value: T) {
    if !set.remove(&value) {
        set.insert(value);
    }
}
