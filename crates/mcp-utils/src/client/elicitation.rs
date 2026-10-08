use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use tokio::sync::oneshot;

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
