use crate::history::{ProjectHistory, SavedSession};
use crate::registry::{Collapsed, State};
use crate::remotes::{Fleet, Instance, RemoteSpec, short_id};
use crate::threads::{Thread, ThreadRef};
use agent_client_protocol::schema::v2::SessionId;
use chrono::{DateTime, Duration, Utc};
use gpui::{App, Entity};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub(super) const RECENT: Duration = Duration::days(7);

pub(super) const SAVED_ROWS: usize = 5;

pub(super) struct Listing<'a> {
    pub(super) state: &'a State,
    pub(super) live: &'a [Entity<Thread>],
    pub(super) history: &'a [ProjectHistory],
    pub(super) fleet: &'a Fleet,
    pub(super) cx: &'a App,
}

pub(super) struct Section {
    pub(super) root: PathBuf,
    pub(super) collapsed: bool,
    pub(super) rows: Vec<Row>,
    pub(super) hidden: usize,
}

pub(super) struct RemoteSection {
    pub(super) remote: RemoteSpec,
    pub(super) collapsed: bool,
    pub(super) problem: Option<String>,
    pub(super) rows: Vec<Row>,
}

#[derive(Clone, PartialEq)]
pub(super) struct Row {
    pub(super) key: RowKey,
    pub(super) thread: Option<ThreadRef>,
    pub(super) title: String,
    pub(super) kind: RowKind,
}

#[derive(Clone, PartialEq)]
pub(super) enum RowKind {
    Local { session_id: Option<SessionId>, started_at: Option<DateTime<Utc>>, pinned: bool },
    Cloud { remote: Box<RemoteSpec>, instance_id: Option<String>, instance: Option<Instance> },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    Live(usize),
    Saved(SessionId),
    Instance(String),
}

#[derive(Clone)]
pub(crate) struct SwitcherEntry {
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) thread: ThreadRef,
}

impl Listing<'_> {
    pub(super) fn roots(&self) -> Vec<&Path> {
        self.state.projects.iter().map(PathBuf::as_path).collect()
    }

    pub(super) fn sections(&self, now: DateTime<Utc>, limits: &HashMap<PathBuf, usize>) -> Vec<Section> {
        let cutoff = now - RECENT;
        let live_sessions = self.live_sessions();
        self.roots()
            .into_iter()
            .map(|root| {
                let collapsed = self.state.collapsed.contains(&Collapsed::Project(root.to_path_buf()));
                let mut section = Section { root: root.to_path_buf(), collapsed, rows: Vec::new(), hidden: 0 };
                if collapsed {
                    return section;
                }
                let live = self
                    .live
                    .iter()
                    .filter(|thread| thread.read(self.cx).project() == Some(root))
                    .map(|thread| self.live_row(thread));
                let saved = self
                    .saved_sessions(root)
                    .filter(|session| {
                        !live_sessions.contains(&session.id) && !self.state.archived.contains(&session.id)
                    })
                    .filter(|session| {
                        session.created_at.is_some_and(|created_at| created_at >= cutoff)
                            || self.state.pinned.contains(&session.id)
                    })
                    .map(|session| self.saved_row(session));
                let limit = limits.get(root).copied().unwrap_or(SAVED_ROWS);
                (section.rows, section.hidden) = visible(live.chain(saved).collect(), limit);
                section
            })
            .collect()
    }

    pub(super) fn remotes(&self) -> Vec<RemoteSection> {
        let inventory = self.fleet.inventory();
        inventory
            .remotes()
            .iter()
            .map(|remote| {
                let collapsed = self.state.collapsed.contains(&Collapsed::Remote(remote.name.clone()));
                let problem = inventory.problem(&remote.name).map(str::to_owned);
                let rows = if collapsed { Vec::new() } else { self.cloud_rows(remote) };
                RemoteSection { remote: remote.clone(), collapsed, problem, rows }
            })
            .collect()
    }

    pub(super) fn unlisted_projects(&self, limit: usize) -> Vec<PathBuf> {
        let listed = self.roots();
        self.history
            .iter()
            .map(|project| project.root.clone())
            .filter(|root| !listed.contains(&root.as_path()))
            .take(limit)
            .collect()
    }

    pub(super) fn inactive_sessions(&self, root: &Path) -> Vec<SessionId> {
        let live_sessions = self.live_sessions();
        self.saved_sessions(root)
            .map(|session| &session.id)
            .filter(|session_id| !live_sessions.contains(*session_id) && !self.state.pinned.contains(*session_id))
            .cloned()
            .collect()
    }

    pub(super) fn switcher_entries(&self, live_order: &[Entity<Thread>]) -> Vec<SwitcherEntry> {
        let live = live_order.iter().map(|entity| {
            let thread = entity.read(self.cx);
            let project =
                thread.target().remote_name().map(str::to_owned).or_else(|| thread.project().map(project_name));
            SwitcherEntry {
                title: self.state.title(thread.session_id(), thread.title()),
                project: project.unwrap_or_default(),
                thread: ThreadRef::Live(entity.clone()),
            }
        });
        let live_sessions = self.live_sessions();
        let saved = self.history.iter().flat_map(|project| {
            project.sessions.iter().filter(|session| !live_sessions.contains(&session.id)).map(|session| {
                SwitcherEntry {
                    title: self.state.title(Some(&session.id), session.title.clone()),
                    project: project_name(&project.root),
                    thread: saved_thread(session),
                }
            })
        });
        live.chain(saved).collect()
    }

    fn cloud_rows(&self, remote: &RemoteSpec) -> Vec<Row> {
        let listed = self.fleet.instances(&remote.name);
        let on_remote: Vec<(&Entity<Thread>, &Thread)> = self
            .live
            .iter()
            .map(|entity| (entity, entity.read(self.cx)))
            .filter(|(_, thread)| thread.target().remote_name() == Some(&remote.name))
            .collect();
        let threads = on_remote.iter().map(|(entity, thread)| {
            let instance_id = thread.target().instance();
            Row {
                key: RowKey::Live(thread.number()),
                thread: Some(ThreadRef::Live((*entity).clone())),
                title: self.state.title(thread.session_id(), thread.title()),
                kind: RowKind::Cloud {
                    remote: Box::new(remote.clone()),
                    instance_id: instance_id.map(str::to_owned),
                    instance: listed.iter().find(|instance| Some(instance.id.as_str()) == instance_id).cloned(),
                },
            }
        });
        let unattached = listed
            .iter()
            .filter(|_| !remote.is_static())
            .filter(|instance| {
                on_remote.iter().all(|(_, thread)| thread.target().instance() != Some(instance.id.as_str()))
            })
            .map(|instance| Row {
                key: RowKey::Instance(instance.id.clone()),
                thread: None,
                title: short_id(&instance.id).to_owned(),
                kind: RowKind::Cloud {
                    remote: Box::new(remote.clone()),
                    instance_id: Some(instance.id.clone()),
                    instance: Some(instance.clone()),
                },
            });
        threads.chain(unattached).collect()
    }

    fn live_row(&self, entity: &Entity<Thread>) -> Row {
        let thread = entity.read(self.cx);
        let session_id = thread.session_id().cloned();
        let key = RowKey::Live(thread.number());
        self.local_row(key, ThreadRef::Live(entity.clone()), session_id, thread.title(), Some(thread.opened_at()))
    }

    fn saved_row(&self, session: &SavedSession) -> Row {
        let key = RowKey::Saved(session.id.clone());
        let session_id = Some(session.id.clone());
        self.local_row(key, saved_thread(session), session_id, session.title.clone(), session.created_at)
    }

    fn local_row(
        &self,
        key: RowKey,
        thread: ThreadRef,
        session_id: Option<SessionId>,
        title: String,
        started_at: Option<DateTime<Utc>>,
    ) -> Row {
        let pinned = session_id.as_ref().is_some_and(|id| self.state.pinned.contains(id));
        let title = self.state.title(session_id.as_ref(), title);
        Row { key, thread: Some(thread), title, kind: RowKind::Local { session_id, started_at, pinned } }
    }

    fn saved_sessions(&self, root: &Path) -> impl Iterator<Item = &SavedSession> {
        self.history.iter().filter(move |project| project.root == root).flat_map(|project| &project.sessions)
    }

    fn live_sessions(&self) -> HashSet<SessionId> {
        self.live.iter().filter_map(|thread| thread.read(self.cx).session_id().cloned()).collect()
    }
}

impl Row {
    pub(super) fn live(&self) -> Option<&Entity<Thread>> {
        match &self.thread {
            Some(ThreadRef::Live(thread)) => Some(thread),
            Some(ThreadRef::Saved { .. }) | None => None,
        }
    }

    pub(super) fn session_id(&self) -> Option<&SessionId> {
        match &self.kind {
            RowKind::Local { session_id, .. } => session_id.as_ref(),
            RowKind::Cloud { .. } => None,
        }
    }
}

impl RowKey {
    pub(crate) fn selector(&self) -> String {
        match self {
            Self::Live(number) => format!("thread-row-{number}"),
            Self::Saved(session_id) => format!("history-row-{}", session_id.0),
            Self::Instance(id) => format!("instance-row-{id}"),
        }
    }
}

pub(super) fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(|| root.display().to_string(), |name| name.to_string_lossy().into_owned())
}

fn visible(rows: Vec<Row>, limit: usize) -> (Vec<Row>, usize) {
    let (mut visible, unpinned): (Vec<Row>, Vec<Row>) =
        rows.into_iter().partition(|row| matches!(row.kind, RowKind::Local { pinned: true, .. }));
    let mut saved = 0;
    let mut hidden = 0;
    for row in unpinned {
        if matches!(row.thread, Some(ThreadRef::Saved { .. })) {
            saved += 1;
            if saved > limit {
                hidden += 1;
                continue;
            }
        }
        visible.push(row);
    }
    (visible, hidden)
}

fn saved_thread(session: &SavedSession) -> ThreadRef {
    ThreadRef::Saved { session_id: session.id.clone(), cwd: session.cwd.clone() }
}
