use std::time::Duration;

use aether_core::context::CompactionConfig;
use aether_core::core::RetryConfig;
use aether_core::testing::{FakeMcpServer, FakeTool, TestScenario, test_agent};
use llm::ProviderError;
use llm::testing::llm_response;

#[tokio::test]
async fn tool_iterations_share_identity_but_next_user_turn_does_not() {
    let result = test_agent()
        .fake_mcp_server("tools", FakeMcpServer::new().with_tool(FakeTool::new("read")))
        .llm_responses(&[
            llm_response("first").tool_call("call-1", "tools__read", &["{}"]).build(),
            llm_response("second").text(&["done"]).build(),
            llm_response("third").text(&["next"]).build(),
        ])
        .scenario(TestScenario::new().user_text("first").wait_for_turn_end().user_text("second").wait_for_turn_end())
        .run_with_context()
        .await
        .unwrap();
    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 3);
    assert!(contexts[0].turn_id().is_some());
    assert_eq!(contexts[0].turn_id(), contexts[1].turn_id());
    assert_ne!(contexts[1].turn_id(), contexts[2].turn_id());
}

#[tokio::test]
async fn cancellation_and_replacement_start_fresh_turn_identities() {
    for replace in [false, true] {
        let blocked = std::sync::Arc::new(tokio::sync::Notify::new());
        let mut scenario = TestScenario::new().user_text("first").wait_for(|event| {
            matches!(
                event,
                aether_core::events::AgentEvent::Message(aether_core::events::MessageEvent::Text {
                    is_complete: false,
                    ..
                })
            )
        });
        scenario = if replace {
            scenario.replace_conversation(vec![llm::ChatMessage::user("replacement")])
        } else {
            scenario.cancel()
        };
        let result = test_agent()
            .without_mcp()
            .llm_responses(&[
                llm_response("one").text(&["partial", "blocked"]).build(),
                llm_response("two").text(&["done"]).build(),
            ])
            .pause_turn_after(0, 2, blocked)
            .scenario(scenario.wait_for_turn_end().user_text("next").wait_for_turn_end())
            .run_with_context()
            .await
            .unwrap();
        let contexts = result.captured_contexts.lock().unwrap();
        assert_eq!(contexts.len(), 2);
        assert!(contexts[0].turn_id().is_some());
        assert_ne!(contexts[0].turn_id(), contexts[1].turn_id());
    }
}

#[tokio::test]
async fn compaction_preserves_active_turn_identity() {
    let result = test_agent()
        .without_mcp()
        .llm_responses(&[
            llm_response("summary").text(&["summary"]).build(),
            llm_response("answer").text(&["hello"]).build(),
        ])
        .context_window_override(100)
        .compaction_config(CompactionConfig::with_threshold(0.85))
        .messages(vec![llm::ChatMessage::user("x".repeat(400))])
        .user_text("go")
        .run_with_context()
        .await
        .unwrap();
    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert!(contexts[0].turn_id().is_some());
    assert_eq!(contexts[0].turn_id(), contexts[1].turn_id());
}

#[tokio::test(start_paused = true)]
async fn retry_keeps_turn_identity() {
    let result = test_agent()
        .without_mcp()
        .retry_config(RetryConfig { max_attempts: 1, base_delay: Duration::ZERO, max_delay: Duration::ZERO })
        .llm_result_responses(&[
            vec![Err(ProviderError::server("retry").into())],
            llm_response("retry").text(&["done"]).build().into_iter().map(Ok).collect(),
        ])
        .user_text("go")
        .run_with_context()
        .await
        .unwrap();
    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts.len(), 2);
    assert!(contexts[0].turn_id().is_some());
    assert_eq!(contexts[0].turn_id(), contexts[1].turn_id());
}
