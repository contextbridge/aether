//! Past sessions read from `aether-sessions`, grouped by the project they ran in.

use aether_sessions::{SessionStore, SessionSummary};
use chrono::{DateTime, Utc};
use gpui::{AppContext, Context, Task};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The title of a session that has no prompt to name it yet.
pub const UNTITLED: &str = "New thread";

/// The saved sessions on disk, grouped by project and refreshed off the UI thread.
pub struct History {
    store: Option<Arc<SessionStore>>,
    projects: Vec<ProjectHistory>,
    refresh: Option<Task<()>>,
}

/// The saved sessions of one project, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectHistory {
    pub root: PathBuf,
    pub sessions: Vec<HistoryEntry>,
}

/// One saved session, as the sidebar lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub session_id: String,
    pub cwd: PathBuf,
    pub title: String,
    /// `None` when the session's timestamp cannot be parsed.
    pub created_at: Option<DateTime<Utc>>,
}

impl History {
    /// Reads history from `store`, or shows none when there is no store.
    pub fn new(store: Option<SessionStore>) -> Self {
        Self { store: store.map(Arc::new), projects: Vec::new(), refresh: None }
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
        let read = cx.background_spawn(async move { group_by_project(store.list(), git_root) });
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
pub fn group_by_project(sessions: Vec<SessionSummary>, project_root: impl Fn(&Path) -> PathBuf) -> Vec<ProjectHistory> {
    let mut projects: Vec<ProjectHistory> = Vec::new();
    for summary in sessions {
        let root = project_root(&summary.meta.cwd);
        let entry = HistoryEntry::from(summary);
        match projects.iter_mut().find(|project| project.root == root) {
            Some(project) => project.sessions.push(entry),
            None => projects.push(ProjectHistory { root, sessions: vec![entry] }),
        }
    }
    for project in &mut projects {
        project.sessions.sort_by(newest_first);
    }
    projects.sort_by(|a, b| newest_first(&a.sessions[0], &b.sessions[0]));
    projects
}

/// The root of the git checkout `dir` is in, or `dir` itself outside of one.
pub fn git_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|ancestor| ancestor.join(".git").exists()).unwrap_or(dir).to_path_buf()
}

impl From<SessionSummary> for HistoryEntry {
    fn from(summary: SessionSummary) -> Self {
        let SessionSummary { meta, title } = summary;
        Self {
            created_at: DateTime::parse_from_rfc3339(&meta.created_at).ok().map(|time| time.to_utc()),
            title: title.filter(|title| !title.trim().is_empty()).unwrap_or_else(|| UNTITLED.to_owned()),
            session_id: meta.session_id,
            cwd: meta.cwd,
        }
    }
}

fn newest_first(a: &HistoryEntry, b: &HistoryEntry) -> std::cmp::Ordering {
    b.created_at.cmp(&a.created_at).then_with(|| b.session_id.cmp(&a.session_id))
}
