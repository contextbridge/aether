use agent_client_protocol::Responder;
use agent_client_protocol::schema::v2::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationContentValue,
};
use std::collections::BTreeMap;

/// The answer to one elicitation, sent at most once.
///
/// Answering consumes the responder. Dropping it unanswered cancels the
/// request, because an ACP [`Responder`] dropped without a reply leaves the
/// agent waiting forever: a prompt that is dismissed, replaced, or torn down
/// with its thread must still tell the agent the user is not coming.
// Boxed because an ACP responder is large, and hosts keep this in enums of
// prompts and effects that would all grow to its size.
#[derive(Debug)]
pub struct ElicitationResponder(Option<Box<Responder<CreateElicitationResponse>>>);

impl ElicitationResponder {
    pub fn new(responder: Responder<CreateElicitationResponse>) -> Self {
        Self(Some(Box::new(responder)))
    }

    /// Accepts the request with `content`, which a URL elicitation leaves
    /// empty.
    pub fn accept(self, content: Option<BTreeMap<String, ElicitationContentValue>>) {
        self.respond(ElicitationAction::Accept(ElicitationAcceptAction::new().content(content)));
    }

    /// Accepts the request with string answers keyed by field name.
    pub fn accept_strings<'a, 'b>(self, content: impl IntoIterator<Item = (&'a str, &'b str)>) {
        self.accept(Some(
            content
                .into_iter()
                .map(|(name, value)| (name.to_string(), ElicitationContentValue::String(value.to_string())))
                .collect(),
        ));
    }

    /// The user saw the request and refused it.
    pub fn decline(self) {
        self.respond(ElicitationAction::Decline);
    }

    /// The user dismissed the request without answering it.
    pub fn cancel(self) {
        self.respond(ElicitationAction::Cancel);
    }

    fn respond(mut self, action: ElicitationAction) {
        send(&mut self.0, action);
    }
}

impl Drop for ElicitationResponder {
    fn drop(&mut self) {
        send(&mut self.0, ElicitationAction::Cancel);
    }
}

impl From<Responder<CreateElicitationResponse>> for ElicitationResponder {
    fn from(responder: Responder<CreateElicitationResponse>) -> Self {
        Self::new(responder)
    }
}

fn send(responder: &mut Option<Box<Responder<CreateElicitationResponse>>>, action: ElicitationAction) {
    if let Some(responder) = responder.take() {
        // The only failure is a connection that is already gone, and then
        // nobody is left waiting for the answer.
        let _ = responder.respond(CreateElicitationResponse::new(action));
    }
}
