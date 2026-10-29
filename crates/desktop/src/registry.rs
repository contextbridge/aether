use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_ui_core::ThemePreference;
use gpui::{AppContext, Context, Entity, Pixels, Size, Subscription};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use utils::fs::{read_json, write_json_atomic};

pub const STATE_FILE: &str = "state.json";

pub struct Registry {
    state: State,
    saver: Option<Arc<Saver>>,
    _quit: Subscription,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub projects: Vec<PathBuf>,
    pub collapsed: HashSet<Collapsed>,
    pub last_agents: HashMap<PathBuf, String>,
    pub pinned: HashSet<SessionId>,
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

impl Registry {
    pub fn load(file: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let state = file.as_deref().map(read_state).unwrap_or_default();
        let saver = file.map(|file| Arc::new(Saver { file, pending: Mutex::default(), writing: Mutex::default() }));
        let quit = cx.on_app_quit(|this: &mut Self, _| {
            if let Some(saver) = &this.saver {
                *lock(&saver.pending) = Some(this.state.clone());
                saver.save();
            }
            async {}
        });
        Self { state, saver, _quit: quit }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn update(registry: &Entity<Self>, cx: &mut impl AppContext, change: impl FnOnce(&mut State)) {
        registry.update(cx, |this, cx| {
            let before = this.state.clone();
            change(&mut this.state);
            if this.state == before {
                return;
            }
            cx.notify();
            if let Some(saver) = this.saver.clone() {
                *lock(&saver.pending) = Some(this.state.clone());
                cx.background_spawn(async move { saver.save() }).detach();
            }
        });
    }
}

impl State {
    pub fn add_project(&mut self, root: PathBuf) {
        if !self.projects.contains(&root) {
            self.projects.push(root);
        }
    }

    pub fn toggle_pinned(&mut self, session_id: SessionId) {
        toggle(&mut self.pinned, session_id);
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

struct Saver {
    file: PathBuf,
    pending: Mutex<Option<State>>,
    writing: Mutex<()>,
}

impl Saver {
    fn save(&self) {
        let _writing = lock(&self.writing);
        let Some(state) = lock(&self.pending).take() else {
            return;
        };
        if let Err(error) = write_json_atomic(&self.file, &state) {
            tracing::warn!(%error, "could not save the desktop state");
        }
    }
}

fn toggle<T: Eq + std::hash::Hash>(set: &mut HashSet<T>, value: T) {
    if !set.remove(&value) {
        set.insert(value);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn read_state(file: &Path) -> State {
    read_json::<State>(file)
        .inspect_err(|error| tracing::warn!(%error, "starting with no saved desktop state"))
        .ok()
        .flatten()
        .unwrap_or_default()
}
