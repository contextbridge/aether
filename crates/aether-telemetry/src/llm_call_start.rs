use aether_core::events::LlmCallPurpose;
use std::time::Duration;

/// Borrowed view of [`AgentEvent::LlmCallStarted`](aether_core::events::AgentEvent::LlmCallStarted);
/// named fields keep the two optional strings from being swapped at the call site.
#[derive(Clone, Copy)]
pub(crate) struct LlmCallStart<'a> {
    pub(crate) purpose: LlmCallPurpose,
    pub(crate) provider: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) display_name: &'a str,
    pub(crate) attempt: u32,
    pub(crate) delay: Option<Duration>,
}
