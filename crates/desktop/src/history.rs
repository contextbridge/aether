//! Past sessions read from `aether-sessions`, grouped by the project they ran in.

use aether_sessions::{SessionStore, SessionSummary, SummaryCache};
use chrono::{DateTime, Utc};
use gpui::{AppContext, Context, Task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

/// The title of a session that has no prompt to name it yet.
pub const UNTITLED: &str = "New thread";

/// The saved sessions on disk, grouped by project and refreshed off the UI thread.
pub struct History {
    store: Option<Arc<SessionStore>>,
    /// What earlier refreshes read, so that a refresh reads only the sessions that changed.
    summaries: Arc<Mutex<SummaryCache>>,
    projects: Vec<ProjectHistory>,
    refresh: Option<Task<()>>,
}

/// The saved sessions of one project, newest first.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectHistory {
    pub root: PathBuf,
    pub sessions: Vec<SessionSummary>,
}

impl History {
    pub fn new(store: Option<SessionStore>) -> Self {
        Self { store: store.map(Arc::new), summaries: Arc::default(), projects: Vec::new(), refresh: None }
    }

    /// The projects that have saved sessions, the most recently active first.
    pub fn projects(&self) -> &[ProjectHistory] {
        &self.projects
    }

    /// Re-reads the store in the background. A refresh still running is superseded.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store.clone() else {
            return;
        };
        let summaries = self.summaries.clone();
        let read = cx.background_spawn(async move {
            let sessions = store.list_cached(&mut summaries.lock().unwrap_or_else(PoisonError::into_inner));
            let mut roots = HashMap::new();
            group_by_project(sessions, |cwd| roots.entry(cwd.to_path_buf()).or_insert_with(|| git_root(cwd)).clone())
        });
        self.refresh = Some(cx.spawn(async move |this, cx| {
            let projects = read.await;
            let _ = this.update(cx, |this, cx| {
                this.projects = projects;
                cx.notify();
            });
        }));
    }
}

/// Groups `sessions` by the project `project_root` maps each session's directory to.
///
/// Sessions are ordered newest first within each project, and projects by their newest session.
/// Recency is the session's creation time, as `aether-sessions` records nothing later.
pub fn group_by_project(
    mut sessions: Vec<SessionSummary>,
    mut project_root: impl FnMut(&Path) -> PathBuf,
) -> Vec<ProjectHistory> {
    sessions.sort_by(|a, b| created_at(b).cmp(&created_at(a)).then_with(|| b.meta.session_id.cmp(&a.meta.session_id)));
    let mut projects: Vec<ProjectHistory> = Vec::new();
    for summary in sessions {
        let root = project_root(&summary.meta.cwd);
        match projects.iter_mut().find(|project| project.root == root) {
            Some(project) => project.sessions.push(summary),
            None => projects.push(ProjectHistory { root, sessions: vec![summary] }),
        }
    }
    projects
}

/// The root of the git checkout `dir` is in, or `dir` itself outside of one.
pub fn git_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|ancestor| ancestor.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

/// When the session was created, or `None` when its timestamp cannot be parsed.
pub fn created_at(summary: &SessionSummary) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&summary.meta.created_at).ok().map(|time| time.to_utc())
}

/// The session's title, or [`UNTITLED`] when it has no prompt to name it.
pub fn title(summary: &SessionSummary) -> String {
    summary.title.clone().filter(|title| !title.trim().is_empty()).unwrap_or_else(|| UNTITLED.to_owned())
}
