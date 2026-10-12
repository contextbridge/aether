use rmcp::model::{
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationCapability, FormElicitationCapability,
    UrlElicitationCapability,
};
use tokio::sync::{mpsc, oneshot};

#[derive(Debug, Clone)]
pub struct ElicitationSink {
    pub(super) events: mpsc::Sender<Elicitation>,
    pub(super) capability: ElicitationCapability,
}

#[derive(Debug)]
pub enum Elicitation {
    Request(Box<ElicitationRequest>),
    Complete { server: String, id: String },
}

#[derive(Debug)]
pub struct ElicitationRequest {
    pub server: String,
    pub request: ElicitRequestParams,
    responder: oneshot::Sender<ElicitResult>,
}

impl ElicitationSink {
    pub fn new(events: mpsc::Sender<Elicitation>, capability: ElicitationCapability) -> Self {
        Self { events, capability }
    }

    pub fn form_and_url(events: mpsc::Sender<Elicitation>) -> Self {
        let capability = ElicitationCapability::new()
            .with_form(FormElicitationCapability::new())
            .with_url(UrlElicitationCapability::new());
        Self::new(events, capability)
    }
}

impl ElicitationRequest {
    pub fn respond(self, result: ElicitResult) {
        let _ = self.responder.send(result);
    }

    pub(crate) fn new(server: String, request: ElicitRequestParams) -> (Self, oneshot::Receiver<ElicitResult>) {
        let (responder, response) = oneshot::channel();
        (Self { server, request, responder }, response)
    }
}

pub(crate) fn cancelled() -> ElicitResult {
    ElicitResult::new(ElicitationAction::Cancel)
}
