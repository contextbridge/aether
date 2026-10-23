use crate::command::Command;
use crate::composer::submission::Submission;
use crate::elicitation::ElicitationResponder;
use agent_client_protocol::schema::v2::CreateElicitationRequest;
use utils::artifact_review::ArtifactReviewElicitationMeta;

/// Something a [`SessionCore`](super::SessionCore) transition asks of its host.
///
/// Hosts apply effects in the order the core produced them. Every piece of state an effect
/// announces has already been updated on the core, so a host reacting to one reads the new state.
#[derive(Debug)]
pub enum Effect {
    /// Work for the host's command dispatcher. Widen it with [`Command::widen`] to dispatch it
    /// alongside the host's own commands.
    Dispatch(Command),
    /// The agent asks the user something. The host owns the prompt until it answers through
    /// `responder`; dropping the responder cancels it.
    ShowElicitation {
        elicitation: Elicitation,
        responder: ElicitationResponder,
    },
    /// The agent's turn ended on its own, with no prompt queued behind it.
    TurnFinished,
    ConfigOptionsChanged,
    AuthMethodsChanged,
    ServerStatusesChanged,
    /// A prompt never reached the agent; give its submission back to the user.
    RestoreSubmission(Submission),
    /// A new, resumed or moved session replaced the conversation.
    SessionChanged,
    /// A git review was sent to the agent as a prompt.
    ReviewSubmitted,
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
