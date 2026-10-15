use aether_core::testing::{TestScenario, test_agent};

#[test]
#[should_panic(expected = "mutually exclusive")]
fn builder_rejects_conflicting_execution_policies() {
    let _ = test_agent().user_text("hello").scenario(TestScenario::new().wait_for_turn_end());
}
