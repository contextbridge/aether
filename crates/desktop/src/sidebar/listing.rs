use super::row::{RowAction, RowIcon};
use crate::history::{ProjectHistory, SavedSession};
use crate::registry::{Collapsed, State};
use crate::remotes::{
    EXPIRY_WARNING, Fleet, Instance, InstanceState, Lifecycle, RemoteSpec, short_id, time_left, time_left_label,
};
use crate::threads::{Thread, ThreadManager, ThreadRef, ThreadStatus};
use agent_client_protocol::v2::schema::SessionId;
use chrono::{DateTime, Duration, Utc};
use clankerdiff_core::format_age;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub(super) const RECENT: Duration = Duration::days(7);

pub(super) const SAVED_ROWS: usize = 5;

pub(super) struct Listing<'a> {
    pub(super) state: &'a State,
    pub(super) threads: ThreadManager,
    pub(super) history: &'a [ProjectHistory],
    pub(super) fleet: Fleet,
    pub(super) renaming: Option<&'a SessionId>,
    pub(super) now: DateTime<Utc>,
}

#[derive(Clone, PartialEq)]
pub(super) struct Section {
    pub(super) root: PathBuf,
    pub(super) collapsed: bool,
    pub(super) rows: Vec<Row>,
    pub(super) hidden: usize,
}

#[derive(Clone, PartialEq)]
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
    pub(super) icon: RowIcon,
    pub(super) unread: bool,
    pub(super) selected: bool,
    pub(super) actions: Vec<RowAction>,
    pub(super) kind: RowKind,
}

#[derive(Clone, PartialEq)]
pub(super) enum RowKind {
    Local { session_id: Option<SessionId>, age: Option<String>, editing: bool },
    Cloud { remote: Box<RemoteSpec>, instance_id: Option<String>, subtitle: String, expiring: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    Live(usize),
    Saved(SessionId),
    Instance(String),
}

#[derive(Clone)]
pub(crate) struct SwitcherEntry {
    pub(crate) key: RowKey,
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) thread: ThreadRef,
}

impl Listing<'_> {
    pub(super) fn roots(&self) -> Vec<&Path> {
        self.state.projects.iter().map(PathBuf::as_path).collect()
    }

    pub(super) fn sections(&self, limits: &HashMap<PathBuf, usize>) -> Vec<Section> {
        let cutoff = self.now - RECENT;
        let live_sessions = self.live_sessions();
        let live = self.live();
        self.roots()
            .into_iter()
            .map(|root| {
                let collapsed = self.state.collapsed.contains(&Collapsed::Project(root.to_path_buf()));
                let mut section = Section { root: root.to_path_buf(), collapsed, rows: Vec::new(), hidden: 0 };
                if collapsed {
                    return section;
                }
                let live = live
                    .iter()
                    .filter(|thread| thread.project().as_deref() == Some(root))
                    .map(|thread| self.live_row(*thread));
                let saved = self
                    .saved_sessions(root)
                    .filter(|session| {
                        !live_sessions.contains(&session.id) && !self.state.archived.contains(&session.id)
                    })
                    .filter(|session| session.created_at.is_some_and(|created_at| created_at >= cutoff))
                    .map(|session| self.saved_row(session));
                let limit = limits.get(root).copied().unwrap_or(SAVED_ROWS);
                (section.rows, section.hidden) = visible(live.chain(saved).collect(), limit);
                section
            })
            .collect()
    }

    pub(super) fn remotes(&self) -> Vec<RemoteSection> {
        self.fleet
            .with_inventory(|inventory| {
                inventory
                    .remotes()
                    .iter()
                    .map(|remote| {
                        let collapsed = self.state.collapsed.contains(&Collapsed::Remote(remote.name.clone()));
                        let problem = inventory.problem(&remote.name).map(str::to_owned);
                        (remote.clone(), collapsed, problem)
                    })
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .map(|(remote, collapsed, problem)| {
                let rows = if collapsed { Vec::new() } else { self.cloud_rows(&remote) };
                RemoteSection { remote, collapsed, problem, rows }
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
            .filter(|session_id| !live_sessions.contains(*session_id))
            .cloned()
            .collect()
    }

    pub(super) fn switcher_entries(&self, live_order: &[Thread]) -> Vec<SwitcherEntry> {
        let live = live_order.iter().map(|thread| {
            let target = thread.target();
            let project =
                target.remote_name().map(str::to_owned).or_else(|| thread.project().as_deref().map(project_name));
            SwitcherEntry {
                key: RowKey::Live(thread.number()),
                title: self.state.title(thread.session_id().as_ref(), thread.title()),
                project: project.unwrap_or_default(),
                thread: ThreadRef::Live(*thread),
            }
        });
        let live_sessions = self.live_sessions();
        let saved = self.history.iter().flat_map(|project| {
            project.sessions.iter().filter(|session| !live_sessions.contains(&session.id)).map(|session| {
                SwitcherEntry {
                    key: RowKey::Saved(session.id.clone()),
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
        let on_remote: Vec<Thread> = self
            .live()
            .into_iter()
            .filter(|thread| thread.target().remote_name() == Some(remote.name.as_str()))
            .collect();
        let threads = on_remote.iter().map(|thread| {
            let target = thread.target();
            let instance_id = target.instance();
            let instance = listed.iter().find(|instance| Some(instance.id.as_str()) == instance_id);
            let key = RowKey::Live(thread.number());
            let title = self.state.title(thread.session_id().as_ref(), thread.title());
            self.cloud_row(key, Some(*thread), title, remote, instance_id, instance)
        });
        let unattached = listed
            .iter()
            .filter(|_| !remote.is_static())
            .filter(|instance| on_remote.iter().all(|thread| thread.target().instance() != Some(instance.id.as_str())))
            .map(|instance| {
                let (key, title) = (RowKey::Instance(instance.id.clone()), short_id(&instance.id).to_owned());
                self.cloud_row(key, None, title, remote, Some(&instance.id), Some(instance))
            });
        threads.collect::<Vec<_>>().into_iter().chain(unattached).collect()
    }

    fn cloud_row(
        &self,
        key: RowKey,
        thread: Option<Thread>,
        title: String,
        remote: &RemoteSpec,
        instance_id: Option<&str>,
        instance: Option<&Instance>,
    ) -> Row {
        let status = thread.map(|thread| thread.status());
        let state = instance.map(|instance| instance.state);
        let stop_instance = instance_id.is_some() && remote.can_change(Lifecycle::Stop);
        let mut actions: Vec<RowAction> =
            thread.filter(|_| !stop_instance).map(|_| RowAction::Stop).into_iter().collect();
        match state {
            Some(InstanceState::Running) if remote.can_change(Lifecycle::Suspend) => actions.push(RowAction::Suspend),
            Some(InstanceState::Suspended) if remote.can_change(Lifecycle::Resume) => actions.push(RowAction::Resume),
            _ => {}
        }
        actions.extend(stop_instance.then_some(RowAction::StopInstance));
        let left = instance.and_then(|instance| time_left(instance, SystemTime::from(self.now)));
        let state_label = match (state, left) {
            (_, Some(left)) => Some(format!("{} left", time_left_label(left))),
            (Some(state), None) => Some(state.label().to_owned()),
            (None, None) => None,
        };
        let subtitle = match (instance_id, thread) {
            (None, _) if status == Some(ThreadStatus::Connecting) => "Launching…".to_owned(),
            (None, _) => "Not launched".to_owned(),
            (Some(_), _) if remote.is_static() => remote.name.clone(),
            (Some(_), None) => state_label.unwrap_or_default(),
            (Some(id), Some(_)) => {
                std::iter::once(short_id(id).to_owned()).chain(state_label).collect::<Vec<_>>().join(" · ")
            }
        };
        let (unread, selected) = self.attention(thread);
        Row {
            key,
            thread: thread.map(ThreadRef::Live),
            title,
            icon: status.map_or(RowIcon::Instance(state), RowIcon::Status),
            unread,
            selected,
            actions,
            kind: RowKind::Cloud {
                remote: Box::new(remote.clone()),
                instance_id: instance_id.map(str::to_owned),
                subtitle,
                expiring: left.is_some_and(|left| left < EXPIRY_WARNING),
            },
        }
    }

    fn live_row(&self, thread: Thread) -> Row {
        let session_id = thread.session_id();
        let key = RowKey::Live(thread.number());
        self.local_row(key, ThreadRef::Live(thread), session_id, thread.title(), Some(thread.opened_at()))
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
        let live = thread.live();
        let mut actions = Vec::new();
        if session_id.is_some() {
            actions.extend([RowAction::Rename, RowAction::Archive]);
        }
        actions.extend(live.map(|_| RowAction::Stop));
        let editing = session_id.is_some() && self.renaming == session_id.as_ref();
        let (unread, selected) = self.attention(live);
        Row {
            key,
            title: self.state.title(session_id.as_ref(), title),
            icon: RowIcon::Status(live.map_or(ThreadStatus::Dormant, |thread| thread.status())),
            unread,
            selected,
            actions,
            kind: RowKind::Local {
                session_id,
                age: started_at.map(|started_at| relative_time(started_at, self.now)),
                editing,
            },
            thread: Some(thread),
        }
    }

    fn attention(&self, thread: Option<Thread>) -> (bool, bool) {
        thread
            .map_or((false, false), |thread| (self.threads.is_unread(thread), self.threads.selected() == Some(thread)))
    }

    fn saved_sessions(&self, root: &Path) -> impl Iterator<Item = &SavedSession> {
        self.history.iter().filter(move |project| project.root == root).flat_map(|project| &project.sessions)
    }

    fn live_sessions(&self) -> HashSet<SessionId> {
        self.live().iter().filter_map(Thread::session_id).collect()
    }

    fn live(&self) -> Vec<Thread> {
        self.threads.threads()
    }
}

impl Row {
    pub(super) fn live(&self) -> Option<Thread> {
        self.thread.as_ref().and_then(ThreadRef::live)
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
    let mut saved = 0;
    let (visible, hidden): (Vec<Row>, Vec<Row>) = rows.into_iter().partition(|row| {
        if !matches!(row.thread, Some(ThreadRef::Saved { .. })) {
            return true;
        }
        saved += 1;
        saved <= limit
    });
    (visible, hidden.len())
}

fn saved_thread(session: &SavedSession) -> ThreadRef {
    ThreadRef::Saved { session_id: session.id.clone(), cwd: session.cwd.clone() }
}

fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    format_age(u64::try_from(now.signed_duration_since(then).num_seconds()).unwrap_or(0))
}
