use super::FakeGit;
use acp_utils::notifications::{GitDiffClosePayload, GitDiffCommandPayload, GitDiffEventPayload};
use agent_client_protocol::schema::v2::SessionId;
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{self as acp, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled};
use clankerdiff_client::protocol::client::ClientCommand;
use clankerdiff_client::protocol::server::{SentDocument, ServerEvent};
use clankerdiff_client::protocol::shared::{Event, LIVE_PROTOCOL_VERSION};
use clankerdiff_client::{RemoteError, RemoteErrorCode};
use clankerdiff_core::{DiffDocument, RepoPath, ReviewTarget};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use tokio::task::AbortHandle;

pub(super) struct FakeGitDiffServer {
    served: Arc<Mutex<Served>>,
    watcher: Option<AbortHandle>,
}

struct Served {
    git: FakeGit,
    session: Option<ServedSession>,
    sent: SentDocument,
}

struct ServedSession {
    target: ReviewTarget,
    published: DiffDocument,
    listed: Arc<[RepoPath]>,
    health: Option<RemoteError>,
}

impl FakeGitDiffServer {
    pub(super) fn new(git: FakeGit) -> Self {
        let served = Served { git, session: None, sent: SentDocument::default() };
        Self { served: Arc::new(Mutex::new(served)), watcher: None }
    }

    fn served(&self) -> MutexGuard<'_, Served> {
        lock(&self.served)
    }

    fn watch(&mut self, session_id: String, cx: ConnectionTo<Client>) {
        let served = self.served.clone();
        let mut changes = self.served().git.subscribe();
        let task = tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let mut served = lock(&served);
                let events = served.poll();
                if served.send(&session_id, events, &cx).is_err() {
                    return;
                }
            }
        });
        self.watcher = Some(task.abort_handle());
    }

    fn stop_watching(&mut self) {
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }
}

impl Served {
    fn on_command(&mut self, command: ClientCommand) -> Vec<ServerEvent> {
        let Some(session) = self.session.as_mut() else {
            return self.initialize(command);
        };
        match command {
            ClientCommand::Initialize { .. } => {
                self.session = None;
                vec![Event::Error(protocol_error("repeated Hello"))]
            }
            ClientCommand::SetTarget(target) => match self.git.load_diff(&target) {
                Ok(document) => {
                    session.target = target;
                    session.published = document.clone();
                    session.health = None;
                    vec![Event::Document(Arc::new(document)), Event::RequestResult(Ok(()))]
                }
                Err(error) => vec![Event::RequestResult(Err(error))],
            },
            ClientCommand::Refresh => {
                let result = self.git.load_diff(&session.target).map(drop);
                let mut events = self.poll();
                events.push(Event::RequestResult(result));
                events
            }
            ClientCommand::Apply(action) => match self.git.apply(action) {
                Ok(()) => {
                    let mut events = self.poll();
                    events.push(Event::RequestResult(Ok(())));
                    events
                }
                Err(error) => vec![Event::RequestResult(Err(error))],
            },
            ClientCommand::Submit(_) | ClientCommand::Cancel => vec![Event::RequestResult(Ok(()))],
            ClientCommand::Lookup { id, request } => vec![Event::Lookup { id, result: self.git.lookup(request) }],
        }
    }

    fn poll(&mut self) -> Vec<ServerEvent> {
        let Some(session) = self.session.as_mut() else {
            return Vec::new();
        };
        match self.git.load_diff(&session.target) {
            Ok(document) => {
                let mut events = Vec::new();
                if document != session.published {
                    session.published = document.clone();
                    events.push(Event::Document(Arc::new(document)));
                }
                let listed = self.git.worktree_files();
                if listed != session.listed {
                    session.listed = listed.clone();
                    events.push(Event::Files(listed));
                }
                if session.health.take().is_some() {
                    events.push(Event::Health { error: None });
                }
                events
            }
            Err(error) if session.health.as_ref() == Some(&error) => Vec::new(),
            Err(error) => {
                session.health = Some(error.clone());
                vec![Event::Health { error: Some(error) }]
            }
        }
    }

    fn initialize(&mut self, command: ClientCommand) -> Vec<ServerEvent> {
        let ClientCommand::Initialize { target, .. } = command else {
            return vec![Event::Error(protocol_error("expected Hello"))];
        };
        let initialize = Event::Initialize {
            protocol_version: LIVE_PROTOCOL_VERSION,
            repository_root: self.git.root().to_string_lossy().into_owned(),
        };
        match self.git.load_diff(&target) {
            Ok(document) => {
                let listed = self.git.worktree_files();
                self.session =
                    Some(ServedSession { target, published: document.clone(), listed: listed.clone(), health: None });
                vec![initialize, Event::Document(Arc::new(document)), Event::Files(listed)]
            }
            Err(error) => vec![initialize, Event::Error(error)],
        }
    }

    fn send(
        &mut self,
        session_id: &str,
        events: Vec<ServerEvent>,
        cx: &ConnectionTo<Client>,
    ) -> Result<(), acp::Error> {
        for event in events {
            let event = self.sent.encode_event(event);
            cx.send_notification(GitDiffEventPayload { session_id: SessionId::new(session_id), event })?;
        }
        Ok(())
    }
}

impl HandleDispatchFrom<Client> for FakeGitDiffServer {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        cx: ConnectionTo<Client>,
    ) -> Result<Handled<Dispatch>, acp::Error> {
        MatchDispatchFrom::new(message, &cx)
            .if_notification(async |payload: GitDiffCommandPayload| {
                let serving = {
                    let mut served = self.served();
                    let events = served.on_command(payload.command);
                    served.send(&payload.session_id, events, &cx)?;
                    served.session.is_some()
                };
                if serving && self.watcher.is_none() {
                    self.watch(payload.session_id, cx.clone());
                }
                Ok(())
            })
            .await
            .if_notification(async |_: GitDiffClosePayload| {
                self.stop_watching();
                let mut served = self.served();
                served.session = None;
                served.sent = SentDocument::default();
                Ok(())
            })
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "FakeGitDiffServer"
    }
}

impl Drop for FakeGitDiffServer {
    fn drop(&mut self) {
        self.stop_watching();
    }
}

fn lock(served: &Mutex<Served>) -> MutexGuard<'_, Served> {
    served.lock().unwrap_or_else(PoisonError::into_inner)
}

fn protocol_error(message: &str) -> RemoteError {
    RemoteError::new(RemoteErrorCode::Protocol, message)
}
