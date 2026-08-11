use rmcp::model::{DEFAULT_MRTR_MAX_ROUNDS, ElicitResult, ElicitationAction, InputRequests, InputRequiredResult};
use std::time::Duration;

#[derive(Debug)]
pub struct MrtrState {
    timeout: Duration,
    input_request_rounds: usize,
    next_backoff: Duration,
    state_only_waited: Duration,
    user_cancelled: bool,
}

#[derive(Debug, PartialEq)]
pub enum MrtrAction {
    /// Sleep for the backoff, then retry with the echoed request state.
    Poll { backoff: Duration, request_state: String },
    /// Dispatch each input request to the user, recording every response via
    /// [`MrtrState::record_response`], then retry with the responses.
    Elicit { input_requests: InputRequests, request_state: Option<String> },
    /// Fail the tool call.
    Abort(AbortReason),
}

#[derive(Debug, PartialEq, Eq)]
pub enum AbortReason {
    EmptyInputRequired,
    PollingBudgetExhausted,
    RePromptAfterCancel,
    InputRoundsExceeded,
}

impl MrtrState {
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            input_request_rounds: 0,
            next_backoff: BASE_BACKOFF,
            state_only_waited: Duration::ZERO,
            user_cancelled: false,
        }
    }

    pub fn tick(&mut self, input_required: InputRequiredResult) -> MrtrAction {
        let input_requests = input_required.input_requests.filter(|requests| !requests.is_empty());
        match (input_requests, input_required.request_state) {
            (None, None) => MrtrAction::Abort(AbortReason::EmptyInputRequired),
            (None, Some(request_state)) => {
                let backoff = self.next_backoff;
                if self.state_only_waited + backoff > self.timeout {
                    MrtrAction::Abort(AbortReason::PollingBudgetExhausted)
                } else {
                    self.state_only_waited += backoff;
                    self.next_backoff = (backoff * 2).min(MAX_BACKOFF);
                    MrtrAction::Poll { backoff, request_state }
                }
            }
            (Some(input_requests), request_state) => {
                if self.user_cancelled {
                    MrtrAction::Abort(AbortReason::RePromptAfterCancel)
                } else if self.input_request_rounds == DEFAULT_MRTR_MAX_ROUNDS {
                    MrtrAction::Abort(AbortReason::InputRoundsExceeded)
                } else {
                    self.input_request_rounds += 1;
                    self.next_backoff = BASE_BACKOFF;
                    self.state_only_waited = Duration::ZERO;
                    MrtrAction::Elicit { input_requests, request_state }
                }
            }
        }
    }

    /// Record an elicitation response so a user cancellation makes the next input-requesting
    /// [`MrtrState::tick`] abort instead of re-prompting.
    pub fn record_response(&mut self, response: &ElicitResult) {
        self.user_cancelled |= response.action == ElicitationAction::Cancel;
    }
}

impl AbortReason {
    pub fn message(&self, server_name: &str, timeout: Duration) -> String {
        match self {
            Self::EmptyInputRequired => {
                format!("Server '{server_name}' requested input without any input requests or request state")
            }
            Self::PollingBudgetExhausted => {
                format!("Server '{server_name}' did not complete within {}s of state-only polling", timeout.as_secs())
            }
            Self::RePromptAfterCancel => {
                format!("Input requested by server '{server_name}' was cancelled by the user")
            }
            Self::InputRoundsExceeded => {
                format!("Server '{server_name}' did not complete within {DEFAULT_MRTR_MAX_ROUNDS} MRTR input rounds")
            }
        }
    }
}

const BASE_BACKOFF: Duration = Duration::from_millis(50);
const MAX_BACKOFF: Duration = Duration::from_millis(1600);

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::{ElicitRequest, ElicitRequestParams, InputRequest};

    const TIMEOUT: Duration = Duration::from_secs(1);

    #[test]
    fn empty_input_required_aborts() {
        let mut test = mrtr();
        let trace = test.run(|_| input_required().build());
        assert!(matches!(trace.last(), Some(MrtrAction::Abort(AbortReason::EmptyInputRequired))));
    }

    #[test]
    fn state_only_polls_with_growing_backoff_until_the_budget_is_spent() {
        let timeout = Duration::from_secs(4);
        let mut test = mrtr().with_timeout(timeout);
        let trace = test.run(|_| input_required().with_request_state("s").build());

        assert_eq!(
            trace.as_slice(),
            &[
                MrtrAction::Poll { backoff: Duration::from_millis(50), request_state: "s".to_string() },
                MrtrAction::Poll { backoff: Duration::from_millis(100), request_state: "s".to_string() },
                MrtrAction::Poll { backoff: Duration::from_millis(200), request_state: "s".to_string() },
                MrtrAction::Poll { backoff: Duration::from_millis(400), request_state: "s".to_string() },
                MrtrAction::Poll { backoff: Duration::from_millis(800), request_state: "s".to_string() },
                MrtrAction::Poll { backoff: Duration::from_millis(1600), request_state: "s".to_string() },
                MrtrAction::Abort(AbortReason::PollingBudgetExhausted),
            ]
        );
    }

    #[test]
    fn an_input_round_refreshes_the_polling_budget() {
        let mut test = mrtr();
        while matches!(test.rounds.tick(input_required().with_request_state("s").build()), MrtrAction::Poll { .. }) {}
        test.elicit_round();

        let decision = test.rounds.tick(input_required().with_request_state("s").build());
        assert!(
            matches!(decision, MrtrAction::Poll { backoff, .. } if backoff == BASE_BACKOFF),
            "budget and backoff should reset after an input round, got {decision:?}"
        );
    }

    #[test]
    fn input_rounds_abort_at_the_cap() {
        let mut test = mrtr();
        let actions = test.run(|_| input_required().with_form().build());
        assert!(
            actions.as_slice()[..DEFAULT_MRTR_MAX_ROUNDS]
                .iter()
                .all(|decision| matches!(decision, MrtrAction::Elicit { .. }))
        );
        assert!(matches!(actions.last(), Some(MrtrAction::Abort(AbortReason::InputRoundsExceeded))));
    }

    #[test]
    fn a_cancelled_round_aborts_the_next_prompt_but_not_state_only_polling() {
        let mut test = mrtr().answering_with(ElicitationAction::Cancel);
        let actions = test.run(|_| input_required().with_form().build());
        assert!(matches!(
            actions.as_slice(),
            [MrtrAction::Elicit { .. }, MrtrAction::Abort(AbortReason::RePromptAfterCancel)]
        ));

        let poll = test.rounds.tick(input_required().with_request_state("s").build());
        assert!(matches!(poll, MrtrAction::Poll { .. }), "the server may still finish up, got {poll:?}");
    }

    #[test]
    fn an_accepted_round_allows_the_next_prompt() {
        let mut test = mrtr().answering_with(ElicitationAction::Accept);
        let elicitation_count = std::cell::Cell::new(0);
        let actions = test.run_until(
            |_| {
                elicitation_count.set(elicitation_count.get() + 1);
                input_required().with_form().build()
            },
            |decision| matches!(decision, MrtrAction::Elicit { .. }) && elicitation_count.get() == 2,
        );

        assert!(matches!(actions.as_slice(), [MrtrAction::Elicit { .. }, MrtrAction::Elicit { .. }]));
    }

    struct MrtrTest {
        rounds: MrtrState,
        response: ElicitResult,
    }

    fn mrtr() -> MrtrTest {
        MrtrTest::default()
    }

    impl Default for MrtrTest {
        fn default() -> Self {
            Self { rounds: MrtrState::new(TIMEOUT), response: ElicitResult::new(ElicitationAction::Accept) }
        }
    }

    impl MrtrTest {
        fn with_timeout(mut self, timeout: Duration) -> Self {
            self.rounds = MrtrState::new(timeout);
            self
        }

        fn answering_with(mut self, action: ElicitationAction) -> Self {
            self.response = ElicitResult::new(action);
            self
        }

        fn run_until<T, U>(&mut self, mut next: T, mut terminal: U) -> Vec<MrtrAction>
        where
            T: FnMut(Option<&MrtrAction>) -> InputRequiredResult,
            U: FnMut(&MrtrAction) -> bool,
        {
            let mut actions = Vec::new();
            loop {
                let action = self.rounds.tick(next(actions.last()));
                if matches!(action, MrtrAction::Elicit { .. }) {
                    self.rounds.record_response(&self.response);
                }
                let is_terminal = terminal(&action);
                actions.push(action);
                if is_terminal {
                    return actions;
                }
            }
        }

        fn run<T>(&mut self, next: T) -> Vec<MrtrAction>
        where
            T: FnMut(Option<&MrtrAction>) -> InputRequiredResult,
        {
            self.run_until(next, |decision| matches!(decision, MrtrAction::Abort(_)))
        }

        fn elicit_round(&mut self) {
            let actions = self.run_until(
                |_| input_required().with_form().build(),
                |decision| matches!(decision, MrtrAction::Elicit { .. }),
            );
            assert!(matches!(actions.last(), Some(MrtrAction::Elicit { .. })));
        }
    }

    fn input_required() -> InputRequiredBuilder {
        InputRequiredBuilder::default()
    }

    #[derive(Default)]
    struct InputRequiredBuilder {
        input_requests: Option<InputRequests>,
        request_state: Option<String>,
    }

    impl InputRequiredBuilder {
        fn with_form(mut self) -> Self {
            let params = ElicitRequestParams::FormElicitationParams {
                meta: None,
                message: "m".to_string(),
                requested_schema: serde_json::from_value(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }))
                .unwrap(),
            };
            let mut requests = InputRequests::new();
            requests.insert("k".to_string(), InputRequest::Elicitation(ElicitRequest::new(params)));
            self.input_requests = Some(requests);
            self
        }

        fn with_request_state(mut self, state: &str) -> Self {
            self.request_state = Some(state.to_string());
            self
        }

        fn build(self) -> InputRequiredResult {
            InputRequiredResult::new(self.input_requests, self.request_state)
        }
    }
}
