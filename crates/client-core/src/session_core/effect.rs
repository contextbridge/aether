use crate::command::Command;
use crate::composer::attachment::PromptAttachment;
use crate::composer::submission::Submission;
use crate::elicitation::ElicitationResponder;
use crate::request::RequestId;
use acp_utils::notifications::{PromptSearchResponse, SessionPreviewResponse, WorkspaceEntry};
use agent_client_protocol::schema::v2::{CreateElicitationRequest, SessionInfo};
use clankerdiff_client::ClientState;
use std::convert::Infallible;
use std::sync::Arc;
use utils::artifact_review::ArtifactReviewElicitationMeta;

/// Something a [`SessionCore`](super::SessionCore) transition asks of its host.
///
/// Effects are returned in the order the transition produced them; hosts apply
/// them in that order. Every piece of state an effect announces has already
/// been updated on the core, so a host reacting to one reads the new state.
#[derive(Debug)]
pub enum Effect {
    /// Work for the host's command dispatcher. Core commands carry no host
    /// part; widen them with [`Command::widen`] to dispatch alongside the
    /// host's own commands.
    Dispatch(Command<Infallible>),
    /// The agent asks the user something. The host owns the prompt until it
    /// answers through `responder`; dropping the responder cancels it.
    ShowElicitation {
        elicitation: Elicitation,
        responder: ElicitationResponder,
    },
    /// The git review client published new state.
    GitReviewState(Arc<ClientState>),
    /// A git review action the host sent finished.
    GitReviewActionFinished(Result<(), String>),
    /// The agent's turn ended on its own, with no prompt queued behind it.
    TurnFinished,
    /// The core appended this client notice to the conversation.
    Notice(String),
    /// The agent's sessions, other than this one, for the host to offer.
    SessionsListed(Vec<SessionInfo>),
    SessionPreviewLoaded {
        session_id: String,
        result: Result<SessionPreviewResponse, String>,
    },
    /// The workspaces a workspace move can target. The core is now waiting
    /// for the host to pick one with
    /// [`SessionCore::move_workspace`](super::SessionCore::move_workspace).
    WorkspacesListed(Vec<WorkspaceEntry>),
    PromptSearchResults {
        query: String,
        result: Result<PromptSearchResponse, String>,
    },
    FilesIndexed {
        request_id: RequestId,
        files: Vec<PromptAttachment>,
    },
    /// The session's config options were replaced by the agent.
    ConfigOptionsChanged,
    AuthMethodsChanged,
    ServerStatusesChanged,
    /// A provider login the host started finished.
    AuthenticationFinished {
        method_id: String,
        result: Result<(), String>,
    },
    /// A prompt never reached the agent; give its submission back to the user.
    RestoreSubmission(Submission),
    /// A new session replaced the previous one.
    SessionCreated,
    /// A session resume was sent; the conversation is empty until it replays.
    ResumeStarted,
    /// A resumed session finished replaying.
    SessionResumed,
    /// A git review was sent to the agent as a prompt.
    ReviewSubmitted,
    /// The session moved to another workspace and is being reloaded there.
    WorkspaceMoved,
    /// The agent connection is gone.
    ConnectionClosed,
}

/// What an elicitation asks of the user.
#[derive(Debug)]
pub enum Elicitation {
    /// An artifact to review, answered with an
    /// [`ArtifactReviewSubmission`](utils::artifact_review::ArtifactReviewSubmission).
    ArtifactReview(ArtifactReviewElicitationMeta),
    /// Any other form or URL elicitation.
    Request(Box<CreateElicitationRequest>),
}
