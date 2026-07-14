use aether_core::events::{Command, UserCommand};
use aether_core::testing::{TestAgentStep, TestScenario, test_agent};

#[test]
fn scenario_steps_build_common_user_commands() {
    let TestAgentStep::Send(Command::UserCommand(UserCommand::Text { content })) = TestAgentStep::user_text("hello")
    else {
        panic!("expected user text command");
    };
    assert_eq!(content, vec![llm::ContentBlock::text("hello")]);

    assert!(matches!(TestAgentStep::cancel(), TestAgentStep::Send(Command::UserCommand(UserCommand::Cancel))));
}

#[test]
#[should_panic(expected = "mutually exclusive")]
fn builder_rejects_conflicting_execution_policies() {
    let _ = test_agent().user_text("hello").scenario(TestScenario::new().wait_for_turn_end());
}
