//! What the app remembers between runs: its projects, what the user made of each thread, the
//! window size, the last selection and the UI preferences.

use crate::theme::ThemePair;
use agent_client_protocol::schema::v2::SessionId;
use gpui::{AppContext, Context, Pixels, Size, Subscription, px, size};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use utils::fs::{read_json, write_json_atomic};

/// The name of the file the registry keeps its state in, inside its state directory.
pub const STATE_FILE: &str = "state.json";

/// The app state kept on disk, in [`default_state_file`] unless the app is given another file.
///
/// Every change is saved off the UI thread as it happens. Saves that pile up while one is
/// written are skipped in favour of the newest, and quitting writes whatever is not yet saved.
/// A missing or unreadable file starts the app with nothing remembered.
pub struct Registry {
    state: State,
    /// `None` when nothing is saved.
    saver: Option<Arc<Saver>>,
    /// Counts the changes made since the registry was loaded.
    generation: u64,
    _quit: Subscription,
}

/// What the user made of one thread. A thread they did nothing with has the default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ThreadMeta {
    /// Pinned threads list above the others in their project.
    pub pinned: bool,
    /// Archived threads list in their project's "Archived" group.
    pub archived: bool,
    /// The title the user gave the thread, shown in place of the one derived from its prompt.
    pub custom_title: Option<String>,
}

impl ThreadMeta {
    /// The custom title if the user gave one, or else `derived`.
    pub fn title_or(&self, derived: String) -> String {
        self.custom_title.clone().unwrap_or(derived)
    }
}

/// Whether a project's threads show below its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disclosure {
    Expanded,
    Collapsed,
}

impl Disclosure {
    pub fn toggled(self) -> Self {
        match self {
            Self::Expanded => Self::Collapsed,
            Self::Collapsed => Self::Expanded,
        }
    }
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
        let saver = file.map(|file| Arc::new(Saver { file, saved: Mutex::new(0), latest: AtomicU64::new(0) }));
        let quit = cx.on_app_quit(|this: &mut Self, _| {
            this.flush();
            async {}
        });
        Self { state, saver, generation: 0, _quit: quit }
    }

    /// The projects added through "Add project", in the order they were added.
    pub fn projects(&self) -> &[PathBuf] {
        &self.state.projects
    }

    /// Adds `root` after the projects already added, unless it is one of them.
    pub fn add_project(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if !self.state.projects.contains(&root) {
            self.state.projects.push(root);
            self.changed(cx);
        }
    }

    /// Whether the user expanded or collapsed the project, if they did.
    pub fn disclosure(&self, root: &Path) -> Option<Disclosure> {
        self.state.disclosures.get(root).copied()
    }

    pub fn set_disclosure(&mut self, root: PathBuf, disclosure: Disclosure, cx: &mut Context<Self>) {
        if self.state.disclosures.insert(root, disclosure) != Some(disclosure) {
            self.changed(cx);
        }
    }

    pub fn thread(&self, session_id: &SessionId) -> &ThreadMeta {
        self.state.threads.get(session_id).unwrap_or(&NO_META)
    }

    /// Titles the thread `title`. A blank title clears the custom title, so the thread shows the
    /// one derived from its prompt again.
    pub fn rename(&mut self, session_id: &SessionId, title: &str, cx: &mut Context<Self>) {
        let title = Some(title.trim()).filter(|title| !title.is_empty()).map(str::to_owned);
        self.update_thread(session_id, |meta| meta.custom_title = title, cx);
    }

    pub fn pin(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.update_thread(session_id, |meta| meta.pinned = true, cx);
    }

    pub fn unpin(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.update_thread(session_id, |meta| meta.pinned = false, cx);
    }

    pub fn archive(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.update_thread(session_id, |meta| meta.archived = true, cx);
    }

    pub fn unarchive(&mut self, session_id: &SessionId, cx: &mut Context<Self>) {
        self.update_thread(session_id, |meta| meta.archived = false, cx);
    }

    /// The size the main window had when last resized.
    pub fn window_size(&self) -> Option<Size<Pixels>> {
        self.state.window.map(|window| size(px(window.width), px(window.height)))
    }

    pub fn set_window_size(&mut self, window: Size<Pixels>, cx: &mut Context<Self>) {
        let window = WindowSize { width: f32::from(window.width), height: f32::from(window.height) };
        if self.state.window.replace(window) != Some(window) {
            self.changed(cx);
        }
    }

    /// The session of the thread that was selected last, if it had one.
    pub fn last_selected(&self) -> Option<&SessionId> {
        self.state.last_selected.as_ref()
    }

    pub fn set_last_selected(&mut self, session_id: Option<SessionId>, cx: &mut Context<Self>) {
        if self.state.last_selected != session_id {
            self.state.last_selected = session_id;
            self.changed(cx);
        }
    }

    /// The themes the user chose in place of the default pair, if they chose any.
    pub fn theme_override(&self) -> Option<&ThemePair> {
        self.state.preferences.theme.as_ref()
    }

    pub fn set_theme_override(&mut self, theme: Option<ThemePair>, cx: &mut Context<Self>) {
        if self.state.preferences.theme != theme {
            self.state.preferences.theme = theme;
            self.changed(cx);
        }
    }

    /// Applies `update` to the thread's metadata, forgetting the thread once nothing is left to
    /// remember about it.
    fn update_thread(&mut self, session_id: &SessionId, update: impl FnOnce(&mut ThreadMeta), cx: &mut Context<Self>) {
        let mut meta = self.thread(session_id).clone();
        update(&mut meta);
        if meta == *self.thread(session_id) {
            return;
        }
        if meta == NO_META {
            self.state.threads.remove(session_id);
        } else {
            self.state.threads.insert(session_id.clone(), meta);
        }
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        cx.notify();
        let Some(saver) = self.saver.clone() else {
            return;
        };
        self.generation += 1;
        let generation = self.generation;
        saver.latest.store(generation, Ordering::Release);
        let state = self.state.clone();
        cx.background_spawn(async move { saver.save(generation, &state) }).detach();
    }

    /// Writes the state now unless it is saved already, since the app may exit before a
    /// background save gets to run.
    fn flush(&self) {
        if let Some(saver) = &self.saver {
            saver.save(self.generation, &self.state);
        }
    }
}

/// The version of the state file this build reads and writes. A file of another version is
/// ignored.
const STATE_VERSION: u32 = 1;

const NO_META: ThreadMeta = ThreadMeta { pinned: false, archived: false, custom_title: None };

/// The state file's contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct State {
    version: u32,
    projects: Vec<PathBuf>,
    disclosures: HashMap<PathBuf, Disclosure>,
    threads: HashMap<SessionId, ThreadMeta>,
    window: Option<WindowSize>,
    last_selected: Option<SessionId>,
    preferences: Preferences,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            projects: Vec::new(),
            disclosures: HashMap::new(),
            threads: HashMap::new(),
            window: None,
            last_selected: None,
            preferences: Preferences::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct WindowSize {
    width: f32,
    height: f32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Preferences {
    theme: Option<ThemePair>,
}

/// Writes snapshots of the state to its file, newest wins.
struct Saver {
    file: PathBuf,
    /// The generation of the snapshot in the file. Held while writing, so writes never overlap.
    saved: Mutex<u64>,
    /// The generation of the newest snapshot handed to a save.
    latest: AtomicU64,
}

impl Saver {
    /// Writes the `generation` snapshot `state`, unless a newer one is written already or waits
    /// to be.
    fn save(&self, generation: u64, state: &State) {
        if generation < self.latest.load(Ordering::Acquire) {
            return;
        }
        let mut saved = self.saved.lock().unwrap_or_else(PoisonError::into_inner);
        if generation <= *saved {
            return;
        }
        match write_json_atomic(&self.file, state) {
            Ok(()) => *saved = generation,
            Err(error) => tracing::warn!(%error, "could not save the desktop state"),
        }
    }
}

fn read_state(file: &Path) -> State {
    match read_json::<State>(file) {
        Ok(Some(state)) if state.version == STATE_VERSION => state,
        Ok(Some(state)) => {
            tracing::warn!(file = %file.display(), version = state.version, "ignoring desktop state of another version");
            State::default()
        }
        Ok(None) => State::default(),
        Err(error) => {
            tracing::warn!(%error, "starting with no saved desktop state");
            State::default()
        }
    }
}
