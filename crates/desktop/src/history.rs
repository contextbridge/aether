use crate::tasks::{blocking, spawn_in};
use aether_sessions::{SessionStore, SessionSummary, SummaryCache};
use agent_client_protocol::v2::schema::SessionId;
use chrono::{DateTime, Utc};
use clankerdiff_rouge_shell::rouge::reactive::{Scope, Signal, Stored, signal, stored};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const UNTITLED: &str = "New thread";

#[derive(Clone, Copy)]
pub struct History {
    store: Stored<Option<Arc<SessionStore>>>,
    summaries: Stored<Option<SummaryCache>>,
    projects: Signal<Vec<ProjectHistory>>,
    refreshing: Stored<Refresh>,
    scope: Scope,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectHistory {
    pub root: PathBuf,
    pub sessions: Vec<SavedSession>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SavedSession {
    pub id: SessionId,
    pub cwd: PathBuf,
    pub created_at: Option<DateTime<Utc>>,
    pub title: String,
}

impl History {
    pub fn new(store: Option<SessionStore>) -> Self {
        Self {
            store: stored(store.map(Arc::new)),
            summaries: stored(Some(SummaryCache::default())),
            projects: signal(Vec::new()),
            refreshing: stored(Refresh::Idle),
            scope: Scope::new(),
        }
    }

    pub fn projects(&self) -> Vec<ProjectHistory> {
        self.projects.get()
    }

    pub fn with_projects<T>(&self, read: impl FnOnce(&[ProjectHistory]) -> T) -> T {
        self.projects.with(|projects| read(projects))
    }

    pub fn refresh(&self) {
        let Some(store) = self.store.get() else {
            return;
        };
        let busy = self.refreshing.update(|refreshing| match refreshing {
            Refresh::Idle => {
                *refreshing = Refresh::Running { again: false };
                false
            }
            Refresh::Running { again } => {
                *again = true;
                true
            }
        });
        if busy {
            return;
        }
        let history = *self;
        let mut summaries = self.summaries.update(Option::take).unwrap_or_default();
        spawn_in(self.scope, async move {
            let read = blocking(move || {
                let sessions = store.list_cached(&mut summaries);
                let mut roots = HashMap::new();
                let projects = group_by_project(sessions, |cwd| {
                    roots.entry(cwd.to_path_buf()).or_insert_with(|| git_root(cwd)).clone()
                });
                (summaries, projects)
            })
            .await;
            if !history.projects.is_alive() {
                return;
            }
            if let Some((summaries, projects)) = read {
                history.summaries.set(Some(summaries));
                history.projects.set_if_changed(projects);
            }
            let again = history.refreshing.update(|refreshing| {
                let again = matches!(refreshing, Refresh::Running { again: true });
                *refreshing = Refresh::Idle;
                again
            });
            if again {
                history.refresh();
            }
        });
    }
}

enum Refresh {
    Idle,
    Running { again: bool },
}

impl From<SessionSummary> for SavedSession {
    fn from(summary: SessionSummary) -> Self {
        let SessionSummary { meta, title } = summary;
        Self {
            id: SessionId::new(meta.session_id.as_str()),
            created_at: DateTime::parse_from_rfc3339(&meta.created_at).ok().map(|time| time.to_utc()),
            cwd: meta.cwd,
            title: title.filter(|title| !title.trim().is_empty()).unwrap_or_else(|| UNTITLED.to_owned()),
        }
    }
}

pub fn group_by_project(
    sessions: Vec<SessionSummary>,
    mut project_root: impl FnMut(&Path) -> PathBuf,
) -> Vec<ProjectHistory> {
    let mut sessions: Vec<SavedSession> = sessions.into_iter().map(SavedSession::from).collect();
    sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at).then_with(|| b.id.0.cmp(&a.id.0)));
    let mut projects: Vec<ProjectHistory> = Vec::new();
    for session in sessions {
        let root = project_root(&session.cwd);
        match projects.iter_mut().find(|project| project.root == root) {
            Some(project) => project.sessions.push(session),
            None => projects.push(ProjectHistory { root, sessions: vec![session] }),
        }
    }
    projects
}

pub fn git_root(dir: &Path) -> PathBuf {
    dir.ancestors().find(|ancestor| ancestor.join(".git").exists()).unwrap_or(dir).to_path_buf()
}
