//! What the app remembers between runs: its projects, what the user made of each thread, the
//! window size, the last selection and the UI preferences.

use agent_client_protocol::schema::v2::SessionId;
use gpui::{AppContext, Context, Entity, Pixels, Size, Subscription};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use utils::fs::{read_json, write_json_atomic};

/// The name of the file the registry keeps its state in, inside its state directory.
pub const STATE_FILE: &str = "state.json";

/// The app state kept on disk, in [`default_state_file`] unless the app is given another file.
///
/// Every change is saved off the UI thread as it happens, the newest change superseding any not
/// yet written, and quitting writes the state once more. A missing or unreadable file starts the
/// app with nothing remembered.
pub struct Registry {
    state: State,
    /// `None` when nothing is saved.
    saver: Option<Arc<Saver>>,
    _quit: Subscription,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// The projects added through "Add project", in the order they were added.
    pub projects: Vec<PathBuf>,
    pub collapsed: HashSet<PathBuf>,
    /// The agent last chosen from each project's new-thread menu, by name.
    pub last_agents: HashMap<PathBuf, String>,
    /// Pinned threads list above the others in their project.
    pub pinned: HashSet<SessionId>,
    /// Archived threads list in their project's "Archived" group.
    pub archived: HashSet<SessionId>,
    /// The titles the user gave threads, shown in place of those derived from their prompts.
    pub titles: HashMap<SessionId, String>,
    pub window_size: Option<Size<Pixels>>,
    pub last_selected: Option<SessionId>,
    /// In pixels per rem.
    pub interface_size: Option<f32>,
    /// In rems, so it scales with the interface size.
    pub sidebar_width: Option<f32>,
}

/// Where the app keeps its state: `~/.aether/desktop/state.json`, or `None` without a home
/// directory.
pub fn default_state_file() -> Option<PathBuf> {
    utils::path::aether_home().map(|home| home.join("desktop").join(STATE_FILE))
}

impl Registry {
    /// Reads the state saved in `file`, which later changes are saved to. Without a file the
    /// registry starts empty and saves nothing.
    pub fn load(file: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let state = file.as_deref().map(read_state).unwrap_or_default();
        let saver = file.map(|file| Arc::new(Saver { file, pending: Mutex::default(), writing: Mutex::default() }));
        let quit = cx.on_app_quit(|this: &mut Self, _| {
            // The app may exit before a background save gets to run.
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

    /// Applies `change` to the registry's state, saving it and notifying observers if it
    /// changed anything.
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
    /// Adds `root` after the projects already added, unless it is one of them.
    pub fn add_project(&mut self, root: PathBuf) {
        if !self.projects.contains(&root) {
            self.projects.push(root);
        }
    }

    /// The title the user gave the thread, or else `derived`.
    pub fn title(&self, session_id: Option<&SessionId>, derived: String) -> String {
        session_id.and_then(|id| self.titles.get(id)).cloned().unwrap_or(derived)
    }

    /// Titles the thread `title`. A blank title clears the custom title, so the thread shows the
    /// one derived from its prompt again.
    pub fn rename(&mut self, session_id: SessionId, title: &str) {
        match title.trim() {
            "" => self.titles.remove(&session_id),
            title => self.titles.insert(session_id, title.to_owned()),
        };
    }
}

/// Writes the newest state handed to it, one write at a time.
struct Saver {
    file: PathBuf,
    pending: Mutex<Option<State>>,
    /// Held while writing, so that an older state is never written over a newer one.
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
