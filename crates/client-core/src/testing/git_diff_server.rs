use super::FakeGit;
use acp_utils::notifications::{GitDiffClosePayload, GitDiffCommandPayload, GitDiffEventPayload};
use agent_client_protocol::schema::v2::SessionId;
use agent_client_protocol::util::MatchDispatchFrom;
use agent_client_protocol::{self as acp, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled};
use clankerdiff_client::protocol::client::ClientCommand;
use clankerdiff_client::protocol::server::{SentDocument, ServerEvent};
use clankerdiff_client::protocol::shared::{Event, LIVE_PROTOCOL_VERSION};
use clankerdiff_core::DiffScope;
use std::sync::Arc;

/// The agent side of `_aether/git_diff` over a [`FakeGit`] repository, chained onto a fake agent
/// with `with_handler`.
///
/// It answers the live diff protocol the way the agent's diff server does: a client that
/// initializes gets the repository's diff for its scope, and every scope change, refresh and
/// repository action is answered with the diff as it now stands, followed by the request's
/// result. Clones of the [`FakeGit`] see the same repository, so a test can change it and have a
/// refresh pick the change up.
pub struct FakeGitDiffServer {
    git: FakeGit,
    scope: DiffScope,
    sent: SentDocument,
}

impl FakeGitDiffServer {
    pub fn new(git: FakeGit) -> Self {
        Self { git, scope: DiffScope::default(), sent: SentDocument::default() }
    }

    fn on_command(&mut self, command: ClientCommand) -> Vec<ServerEvent> {
        match command {
            ClientCommand::Initialize { scope, .. } => {
                self.scope = scope;
                self.sent = SentDocument::default();
                let initialize = Event::Initialize {
                    protocol_version: LIVE_PROTOCOL_VERSION,
                    repository_root: self.git.root().to_string_lossy().into_owned(),
                };
                let document = match self.git.load_diff(self.scope) {
                    Ok(snapshot) => Event::Document(Arc::new(snapshot)),
                    Err(error) => Event::Error(error),
                };
                vec![initialize, document]
            }
            ClientCommand::SetScope(scope) => {
                self.scope = scope;
                vec![self.document(), Event::RequestResult(Ok(()))]
            }
            ClientCommand::Refresh => vec![self.document(), Event::RequestResult(Ok(()))],
            ClientCommand::Apply(action) => {
                let result = self.git.apply(action);
                vec![self.document(), Event::RequestResult(result)]
            }
            ClientCommand::Submit(_) | ClientCommand::Cancel => vec![Event::RequestResult(Ok(()))],
        }
    }

    /// The diff as it now stands, or the reason it cannot be read.
    fn document(&self) -> ServerEvent {
        match self.git.load_diff(self.scope) {
            Ok(snapshot) => Event::Document(Arc::new(snapshot)),
            Err(error) => Event::Health { error: Some(error) },
        }
    }

    fn send(&mut self, session_id: &str, event: ServerEvent, cx: &ConnectionTo<Client>) -> Result<(), acp::Error> {
        let event = self.sent.encode_event(event);
        cx.send_notification(GitDiffEventPayload { session_id: SessionId::new(session_id), event })
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
                for event in self.on_command(payload.command) {
                    self.send(&payload.session_id, event, &cx)?;
                }
                Ok(())
            })
            .await
            .if_notification(async |_: GitDiffClosePayload| {
                self.sent = SentDocument::default();
                Ok(())
            })
            .await
            .done()
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "FakeGitDiffServer"
    }
}
