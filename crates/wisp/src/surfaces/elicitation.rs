use acp_utils::notifications::{ElicitationAction, ElicitationResponse};
use agent_client_protocol::Responder;
use serde_json::Value;

/// Answers one elicitation, exactly once.
///
/// Dropping it unanswered cancels, so tearing navigation down can never leave the
/// agent waiting on a reply that will not come. Every route or overlay that holds an
/// elicitation goes through this instead of writing its own `Drop`.
pub struct ElicitationResponder(Option<Box<dyn FnOnce(ElicitationResponse) + Send>>);

impl ElicitationResponder {
    pub fn new(responder: Responder<ElicitationResponse>) -> Self {
        Self::from_fn(move |response| {
            let _ = responder.respond(response);
        })
    }

    /// For tests, which observe the answer without an ACP connection.
    pub fn from_fn(respond: impl FnOnce(ElicitationResponse) + Send + 'static) -> Self {
        Self(Some(Box::new(respond)))
    }

    /// Sends `action`, or does nothing when this elicitation is already answered.
    pub fn respond(&mut self, action: ElicitationAction, content: Option<Value>) {
        if let Some(respond) = self.0.take() {
            respond(ElicitationResponse { action, content });
        }
    }

    pub fn cancel(&mut self) {
        self.respond(ElicitationAction::Cancel, None);
    }

    pub fn is_answered(&self) -> bool {
        self.0.is_none()
    }
}

impl Drop for ElicitationResponder {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl From<Responder<ElicitationResponse>> for ElicitationResponder {
    fn from(responder: Responder<ElicitationResponse>) -> Self {
        Self::new(responder)
    }
}
