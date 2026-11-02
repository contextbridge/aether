use acp_utils::client::{AcpClientError, AcpEvent};
use acp_utils::conversation::{ConversationContent, Notice, NoticeLevel};
use acp_utils::testing::{Channel, FakeAgent};
use agent_client_protocol::schema::v2::{Implementation, SessionId};
use clankerdiff_client::ClientState;
use clankerdiff_core::DiffDocument;
use client_core::command::CommandResult;
use client_core::connection::{Connection, ConnectionError, Session};
use client_core::runtime::{Input, SessionRuntime};
use client_core::session_core::{Effect, SessionCore};
use client_core::testing::{FakeGit, git_diff_agent};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::task::LocalSet;

/// A `SessionCore` driven by a real `SessionRuntime`, the way a frontend's
/// event loop drives them.
pub(crate) struct LiveSession {
    pub(crate) core: SessionCore,
    pub(crate) runtime: SessionRuntime,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum TestError {
    #[error(transparent)]
    Client(#[from] AcpClientError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl LiveSession {
    pub(crate) async fn connect(
        agent: FakeAgent,
        working_dir: impl Into<PathBuf>,
        session_id: Option<SessionId>,
    ) -> Result<Self, ConnectionError> {
        let connection = Connection::connect_to(client_info(), agent.serve()).await?;
        Ok(Self::start(connection.open(session_id, working_dir.into()).await?))
    }

    pub(crate) async fn reviewing(git: FakeGit) -> Result<Self, ConnectionError> {
        let (agent_end, client_end) = Channel::duplex();
        tokio::task::spawn_local(git_diff_agent(FakeAgent::new("reviewing"), git).connect_to(agent_end));
        let connection = Connection::connect_to(client_info(), client_end).await?;
        Ok(Self::start(connection.open(None, "/repo".into()).await?))
    }

    pub(crate) fn start(session: Session) -> Self {
        let (config, runtime) = session.into_core();
        Self { core: SessionCore::new(config), runtime }
    }

    pub(crate) async fn run_until(&mut self, done: impl Fn(&SessionCore) -> bool) {
        loop {
            for effect in self.core.take_effects() {
                if let Effect::Dispatch(command) = effect {
                    self.runtime.dispatch(command);
                }
            }
            if done(&self.core) {
                return;
            }
            let input = self.runtime.next().await;
            self.core.on_input(input);
        }
    }

    pub(crate) async fn shutdown(mut self) {
        self.runtime.shutdown().await;
    }
}

pub(crate) fn client_info() -> Implementation {
    Implementation::new("client-core-test", "1")
}

/// The fake agent and the ACP client run as local tasks, so every async test
/// runs inside a `LocalSet` on a current-thread runtime.
pub(crate) fn run_local<T: Future>(test: T) -> T::Output {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    LocalSet::new().block_on(&runtime, test)
}

pub(crate) fn deliver(core: &mut SessionCore, result: CommandResult) {
    core.on_input(Input::Result(result));
}

pub(crate) fn receive(core: &mut SessionCore, event: impl Into<AcpEvent>) {
    core.on_input(Input::Event(event.into()));
}

pub(crate) fn notices(core: &SessionCore, level: NoticeLevel) -> Vec<&str> {
    core.conversation()
        .items()
        .iter()
        .filter_map(|item| match item.content() {
            ConversationContent::Notice(Notice { level: shown, text }) if *shown == level => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub(crate) fn offline() -> AcpClientError {
    AcpClientError::Protocol(agent_client_protocol::Error::new(-32603, "offline"))
}

pub(crate) async fn next_result(runtime: &mut SessionRuntime) -> CommandResult {
    loop {
        if let Input::Result(result) = runtime.next().await {
            return result;
        }
    }
}

pub(crate) async fn next_review_state(runtime: &mut SessionRuntime) -> Arc<ClientState> {
    loop {
        if let CommandResult::GitReview(state) = next_result(runtime).await {
            return state;
        }
    }
}

pub(crate) async fn next_document(runtime: &mut SessionRuntime) -> Arc<DiffDocument> {
    loop {
        if let Some(document) = next_review_state(runtime).await.snapshot.clone() {
            return document;
        }
    }
}
