use aether_sessions::{SessionStore, SessionSummary, SummaryCache};
use agent_client_protocol::schema::v2::SessionId;
use chrono::{DateTime, Utc};
use gpui::{AppContext, Context, Task};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

pub const UNTITLED: &str = "New thread";

pub struct History {
    store: Option<Arc<SessionStore>>,
    summaries: Arc<Mutex<SummaryCache>>,
    projects: Vec<ProjectHistory>,
    refresh: Option<Task<()>>,
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
        Self { store: store.map(Arc::new), summaries: Arc::default(), projects: Vec::new(), refresh: None }
    }

    pub fn projects(&self) -> &[ProjectHistory] {
        &self.projects
    }

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
