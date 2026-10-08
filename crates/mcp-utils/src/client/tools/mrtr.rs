use rmcp::model::{DEFAULT_MRTR_MAX_ROUNDS, InputRequests, InputRequiredResult};
use std::time::Duration;
use thiserror::Error;

pub(super) struct MrtrState {
    input_request_rounds: usize,
    next_backoff: Duration,
    user_cancelled: bool,
}

pub(super) enum MrtrAction {
    Poll { backoff: Duration, request_state: String },
    Elicit { input_requests: InputRequests, request_state: Option<String> },
    Abort(AbortReason),
}

#[derive(Debug, PartialEq, Eq, Error)]
pub enum AbortReason {
    #[error("requested input without any input requests or request state")]
    EmptyInputRequired,
    #[error("requested input again after the user cancelled")]
    RePromptAfterCancel,
    #[error("did not complete within {DEFAULT_MRTR_MAX_ROUNDS} MRTR input rounds")]
    InputRoundsExceeded,
}

impl MrtrState {
    pub(super) fn new() -> Self {
        Self { input_request_rounds: 0, next_backoff: BASE_BACKOFF, user_cancelled: false }
    }

    pub(super) fn tick(&mut self, input_required: InputRequiredResult) -> MrtrAction {
        let input_requests = input_required.input_requests.filter(|requests| !requests.is_empty());
        match (input_requests, input_required.request_state) {
            (None, None) => MrtrAction::Abort(AbortReason::EmptyInputRequired),
            (None, Some(request_state)) => {
                let backoff = self.next_backoff;
                self.next_backoff = (backoff * 2).min(MAX_BACKOFF);
                MrtrAction::Poll { backoff, request_state }
            }
            (Some(input_requests), request_state) => {
                if self.user_cancelled {
                    MrtrAction::Abort(AbortReason::RePromptAfterCancel)
                } else if self.input_request_rounds == DEFAULT_MRTR_MAX_ROUNDS {
                    MrtrAction::Abort(AbortReason::InputRoundsExceeded)
                } else {
                    self.input_request_rounds += 1;
                    self.next_backoff = BASE_BACKOFF;
                    MrtrAction::Elicit { input_requests, request_state }
                }
            }
        }
    }

    pub(super) fn record_cancelled(&mut self, cancelled: bool) {
        self.user_cancelled |= cancelled;
    }
}

const BASE_BACKOFF: Duration = Duration::from_millis(50);
const MAX_BACKOFF: Duration = Duration::from_millis(1600);
