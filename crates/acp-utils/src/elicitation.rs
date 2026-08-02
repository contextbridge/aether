//! Boundary between rmcp 3.0's legacy URL elicitation type and Aether's narrow
//! wire representation.
//!
//! MCP 2026-07-28 removed the protocol elicitation id field, but rmcp 3.0 still
//! requires it on `ElicitRequestParams::UrlElicitationParams` (both its struct
//! literal and its deserializer). Aether therefore carries [`ElicitationRequest`]
//! on its own `_aether/elicitation` wire format — a type with no ID — and
//! converts at this single boundary. The conversion ignores the removed field,
//! and `rmcp_url_elicitation` (for fixtures/fake servers) is the only place in
//! Aether sources that names it; no Aether code assigns the value any meaning.

use crate::notifications::{ElicitationRequest, UrlElicitationRequest};
use rmcp::model::ElicitRequestParams;

impl From<ElicitRequestParams> for ElicitationRequest {
    fn from(request: ElicitRequestParams) -> Self {
        match request {
            ElicitRequestParams::FormElicitationParams { meta, message, requested_schema } => {
                ElicitationRequest::Form { meta, message, requested_schema }
            }
            // rmcp 3.0's URL variant still carries the removed protocol
            // elicitation id field; it is dropped here and never forwarded.
            ElicitRequestParams::UrlElicitationParams { message, url, .. } => {
                ElicitationRequest::Url(UrlElicitationRequest { message, url })
            }
            _ => ElicitationRequest::Unsupported,
        }
    }
}

/// Build an rmcp URL elicitation request for fixtures and fake servers. rmcp
/// 3.0 still requires the removed `elicitation_id` field, so the value is a
/// placeholder that Aether never reads, matches on, or forwards.
pub fn rmcp_url_elicitation(message: impl Into<String>, url: impl Into<String>) -> ElicitRequestParams {
    ElicitRequestParams::UrlElicitationParams {
        meta: None,
        message: message.into(),
        url: url.into(),
        elicitation_id: String::new(),
    }
}
