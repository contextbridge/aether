mod form;

pub use form::{
    FieldError, FieldInput, FormError, FormField, FormFieldKind, SelectOption, accept_content, form_fields,
};

use agent_client_protocol::Responder;
use agent_client_protocol::schema::v2::{
    CreateElicitationResponse, ElicitationAcceptAction, ElicitationAction, ElicitationContentValue,
};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use utils::artifact_review::ArtifactReviewElicitationMeta;

#[derive(Debug, Clone)]
pub enum Elicitation {
    Form { source: String, message: String, fields: Vec<FormField> },
    Url { source: String, message: String, url: String },
    ArtifactReview(ArtifactReviewElicitationMeta),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ElicitationId(u64);

#[derive(Debug, Clone, PartialEq)]
pub enum ElicitationAnswer {
    Accept(Option<BTreeMap<String, ElicitationContentValue>>),
    Decline,
    Cancel,
}

impl ElicitationId {
    pub fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

impl Elicitation {
    pub fn question(&self) -> String {
        match self {
            Self::Form { message, .. } | Self::Url { message, .. } => message.clone(),
            Self::ArtifactReview(review) => format!("Review {}", review.title),
        }
    }
}

impl ElicitationAnswer {
    pub fn accept_strings<'a, 'b>(content: impl IntoIterator<Item = (&'a str, &'b str)>) -> Self {
        Self::Accept(Some(
            content
                .into_iter()
                .map(|(name, value)| (name.to_string(), ElicitationContentValue::String(value.to_string())))
                .collect(),
        ))
    }

    fn into_action(self) -> ElicitationAction {
        match self {
            Self::Accept(content) => ElicitationAction::Accept(ElicitationAcceptAction::new().content(content)),
            Self::Decline => ElicitationAction::Decline,
            Self::Cancel => ElicitationAction::Cancel,
        }
    }
}

pub(crate) struct PendingElicitation {
    pub(crate) id: ElicitationId,
    pub(crate) elicitation: Elicitation,
    responder: Option<Responder<CreateElicitationResponse>>,
}

impl PendingElicitation {
    pub(crate) fn new(
        id: ElicitationId,
        elicitation: Elicitation,
        responder: Responder<CreateElicitationResponse>,
    ) -> Self {
        Self { id, elicitation, responder: Some(responder) }
    }

    pub(crate) fn answer(mut self, answer: ElicitationAnswer) {
        self.respond(answer.into_action());
    }

    fn respond(&mut self, action: ElicitationAction) {
        if let Some(responder) = self.responder.take() {
            let _ = responder.respond(CreateElicitationResponse::new(action));
        }
    }
}

impl Drop for PendingElicitation {
    fn drop(&mut self) {
        self.respond(ElicitationAction::Cancel);
    }
}
