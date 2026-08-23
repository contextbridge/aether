use agent_client_protocol::Responder;
use agent_client_protocol::schema::v1::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationContentValue,
};
use std::collections::BTreeMap;

pub struct ElicitationResponder(Option<Box<dyn FnOnce(CreateElicitationResponse) + Send>>);

impl ElicitationResponder {
    pub fn new(responder: Responder<CreateElicitationResponse>) -> Self {
        Self::from_fn(move |response| {
            let _ = responder.respond(response);
        })
    }

    /// For tests, which observe the answer without an ACP connection.
    pub fn from_fn(respond: impl FnOnce(CreateElicitationResponse) + Send + 'static) -> Self {
        Self(Some(Box::new(respond)))
    }

    pub fn accept(&mut self, content: Option<BTreeMap<String, ElicitationContentValue>>) {
        self.respond(ElicitationAction::Accept(ElicitationAcceptAction::new().content(content)));
    }

    pub fn accept_strings<const T: usize>(&mut self, content: [(&str, &str); T]) {
        self.accept(Some(
            content
                .into_iter()
                .map(|(name, value)| (name.to_string(), ElicitationContentValue::String(value.to_string())))
                .collect(),
        ));
    }

    pub fn cancel(&mut self) {
        self.respond(ElicitationAction::Cancel);
    }

    fn respond(&mut self, action: ElicitationAction) {
        if let Some(respond) = self.0.take() {
            respond(CreateElicitationResponse::new(action));
        }
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

impl From<Responder<CreateElicitationResponse>> for ElicitationResponder {
    fn from(responder: Responder<CreateElicitationResponse>) -> Self {
        Self::new(responder)
    }
}
