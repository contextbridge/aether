use aether_core::events::{LlmCallOutcome, MessageEvent, ToolEvent, TurnEvent};
use std::error::Error;
use std::time::Duration;

use aether_core::{
    events::{AgentEvent, Command, TurnOutcome, UserCommand},
    testing::{
        agent_event, content_events, test_agent,
        {AddNumbersRequest, AddNumbersResult, DivideNumbersRequest, SlowToolRequest},
    },
};
use llm::testing::{FakeLlmProvider, llm_response};
use llm::{ChatMessage, ContentBlock, LlmResponse, StopReason};

fn split_json_in_half(input: &str) -> (&str, &str) {
    let split = input.char_indices().nth(input.len() / 2).map_or(1, |(idx, _)| idx).max(1).min(input.len() - 1);
    input.split_at(split)
}

/// Strips turn/call lifecycle noise, leaving only the content events the
/// `agent_event` builder describes.
fn content_events(events: Vec<AgentEvent>) -> Vec<AgentEvent> {
    events
        .into_iter()
        .filter(|event| {
            !matches!(
                event,
                AgentEvent::Turn(
                    TurnEvent::Started { .. }
                        | TurnEvent::RetryScheduled { .. }
                        | TurnEvent::LlmCallStarted { .. }
                        | TurnEvent::LlmCallEnded { .. }
                ) | AgentEvent::Tool(ToolEvent::ExecutionStarted { .. } | ToolEvent::DefinitionsUpdated { .. })
            )
        })
        .collect()
}

#[tokio::test]
async fn test_text_message() -> Result<(), Box<dyn Error>> {
    let id = "message_1";
    let chunks = ["Hello", "user"];
    let llm_responses = [llm_response(id).text(&chunks).build()];
    let mut expected_messages = agent_event(id).text(&chunks).build();
    expected_messages.push(AgentEvent::turn_ended(TurnOutcome::Completed));

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("hi")] })])
        .run()
        .await?;
    assert_eq!(content_events(messages), expected_messages);
    Ok(())
}

#[tokio::test]
async fn test_llm_call_lifecycle_reports_model_and_usage() -> Result<(), Box<dyn Error>> {
    let model: llm::LlmModel = "codex:gpt-5.5".parse()?;
    let llm =
        FakeLlmProvider::new(vec![llm_response("msg_1").text(&["hi"]).usage(120, 7).build()]).with_model(model.clone());
    let (tx, mut rx, _handle) = aether_core::core::agent(llm).spawn().await?;

    tx.send(Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("hello")] })).await?;

    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        let is_turn_end = event.turn_outcome().is_some();
        events.push(event);
        if is_turn_end {
            break;
        }
    }

    let started = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Turn(TurnEvent::LlmCallStarted { provider, model, attempt, .. }) => {
                Some((provider.clone(), model.clone(), *attempt))
            }
            _ => None,
        })
        .expect("LlmCallStarted should be emitted");
    assert_eq!(started.0.as_deref(), Some(model.provider()));
    assert_eq!(started.1.as_deref(), Some(model.model_id().as_ref()));
    assert_eq!(started.2, 0, "initial call should be attempt 0");

    let usage = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::Turn(TurnEvent::LlmCallEnded { outcome: LlmCallOutcome::Completed { usage, .. }, .. }) => {
                *usage
            }
            _ => None,
        })
        .expect("completed LLM call should carry usage");
    assert_eq!(usage.input_tokens, 120);
    assert_eq!(usage.output_tokens, 7);

    Ok(())
}

#[tokio::test]
async fn test_single_tool_call() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(3, 5);
    let tool_result = AddNumbersResult::new(8);
    let (m1_id, t1_id, t1_name) = ("message_1", "call_1", "test__add_numbers");
    let m2_id = "message-2";
    let chunks = ["The", " sum", " is", " 8"];

    let llm_responses = [
        llm_response(m1_id).tool_call(t1_id, t1_name, &[&tool_request.json()?]).build(),
        llm_response(m2_id).text(&chunks).build(),
    ];

    let expected_messages = {
        let mut messages = Vec::new();
        messages.extend(agent_event(m1_id).tool_call(t1_id, t1_name, &tool_request, &tool_result).build());

        messages.extend(agent_event(m2_id).text(&chunks).build());
        messages.push(AgentEvent::turn_ended(TurnOutcome::Completed));
        messages
    };

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("3+5 = ?")],
        })])
        .run()
        .await?;
    assert_eq!(content_events(messages), expected_messages);
    Ok(())
}

#[tokio::test]
async fn test_tool_request_arg_emits_tool_call_update() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(3, 5);
    let request_json = tool_request.json()?;
    let (arg_chunk_1, arg_chunk_2) = split_json_in_half(&request_json);
    let llm_responses =
        [llm_response("message_1").tool_call("call_1", "test__add_numbers", &[arg_chunk_1, arg_chunk_2]).build()];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("3+5 = ?")],
        })])
        .run()
        .await?;

    let tool_call_count = messages
        .iter()
        .filter(|message| {
            matches!(
                message,
                AgentEvent::Tool(ToolEvent::Call { request, .. })
                    if request.id == "call_1" && request.name == "test__add_numbers"
            )
        })
        .count();
    assert_eq!(tool_call_count, 1, "only one start ToolCall should be emitted");

    let update_chunks: Vec<String> = messages
        .iter()
        .filter_map(|message| match message {
            AgentEvent::Tool(ToolEvent::CallUpdate { tool_call_id, chunk, .. }) if tool_call_id == "call_1" => {
                Some(chunk.clone())
            }
            _ => None,
        })
        .collect();

    assert_eq!(update_chunks.len(), 2);
    assert_eq!(update_chunks[0], arg_chunk_1);
    assert_eq!(update_chunks[1], arg_chunk_2);

    assert!(messages.iter().any(|message| {
        matches!(
            message,
            AgentEvent::Tool(ToolEvent::Result { result, .. })
                if result.id == "call_1" && result.result.contains("sum")
        )
    }));

    Ok(())
}

#[tokio::test]
async fn test_tool_call_failure() -> Result<(), Box<dyn Error>> {
    let tool_request = DivideNumbersRequest::new(10, 0);
    let chunks = ["I", " apologize", ",", " but", " division", " by", " zero", " is", " not", " allowed", "."];

    let llm_responses = [
        llm_response("message_1").tool_call("call_1", "test__divide_numbers", &[&tool_request.json()?]).build(),
        llm_response("message_2").text(&chunks).build(),
    ];

    let expected_messages = {
        let mut messages = Vec::new();
        messages.extend(
            agent_event("message_1")
                .tool_call_with_error("call_1", "test__divide_numbers", &tool_request, "Division by zero")
                .build(),
        );

        messages.extend(agent_event("message_2").text(&chunks).build());
        messages.push(AgentEvent::turn_ended(TurnOutcome::Completed));
        messages
    };

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("10 / 0 = ?")],
        })])
        .run()
        .await?;
    assert_eq!(content_events(messages), expected_messages);
    Ok(())
}

#[tokio::test]
async fn test_cancellation() -> Result<(), Box<dyn Error>> {
    let chunks = [
        "This",
        " is",
        " a",
        " long",
        " response",
        " to",
        " ensure",
        " cancellation",
        " happens",
        " during",
        " processing",
    ];

    let llm_responses = [llm_response("message_1").text(&chunks).build()];
    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![
            Command::UserCommand(UserCommand::Text { content: vec![llm::ContentBlock::text("hi")] }),
            Command::UserCommand(UserCommand::Cancel),
        ])
        .run()
        .await?;

    let text_chunks_received =
        messages.iter().filter(|m| matches!(m, AgentEvent::Message(MessageEvent::Text { .. }))).count();

    assert!(
        messages.iter().any(|m| matches!(m.turn_outcome(), Some(TurnOutcome::Cancelled))),
        "Expected the turn to end as cancelled"
    );

    // Due to Agent's merging of N async streams, it's hard to control
    // exact ordering, so we use a coarse grained aseertion here
    assert!(
        text_chunks_received < chunks.len(),
        "Expected cancellation to stop processing before all {} chunks were sent, but received {}",
        chunks.len(),
        text_chunks_received
    );

    Ok(())
}

#[tokio::test]
async fn test_tool_timeout() -> Result<(), Box<dyn Error>> {
    let tool_duration = 2000;
    let tool_timeout = 500;

    let tool_request = SlowToolRequest::new(tool_duration);
    let (m1_id, t1_id, t1_name) = ("message_1", "call_1", "test__slow_tool");

    let llm_responses = [llm_response(m1_id).tool_call(t1_id, t1_name, &[&tool_request.json()?]).build()];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("run slow tool")],
        })])
        .tool_timeout(Duration::from_millis(tool_timeout))
        .run()
        .await?;

    let has_tool_error = messages.iter().any(|m| {
        matches!(
            m,
            AgentEvent::Tool(ToolEvent::Error { error, .. }) if error.error.contains("timeout")
        )
    });

    assert!(has_tool_error, "Expected a ToolError with timeout message, got: {messages:?}");

    Ok(())
}

#[tokio::test]
async fn test_simple_message_content() -> Result<(), Box<dyn Error>> {
    let (id, chunks) = ("message_1", ["Hello"]);
    let llm_responses = [llm_response(id).text(&chunks).build()];

    let result = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("Just a simple message")],
        })])
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    let first_context = &contexts[0];
    let messages = first_context.messages();

    let user_message =
        messages.iter().find(|m| matches!(m, ChatMessage::User { .. })).expect("Expected a user message");

    let ChatMessage::User { content, .. } = user_message else {
        panic!("Expected User message");
    };

    // Content should be exactly the user's message
    assert_eq!(content, &vec![ContentBlock::text("Just a simple message")]);

    Ok(())
}

#[tokio::test]
async fn test_auto_continue_not_triggered_for_end_turn() -> Result<(), Box<dyn Error>> {
    let chunks = ["I have completed the task."];
    let llm_responses = [llm_response("msg_1").text(&chunks).build()];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .max_auto_continues(3)
        .run()
        .await?;

    let auto_continue_count =
        messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))).count();
    assert_eq!(auto_continue_count, 0, "Expected no AutoContinue messages for normal end-turn completion");

    assert!(matches!(messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)));

    Ok(())
}

#[tokio::test]
async fn test_auto_continue_not_triggered_for_opening_message() -> Result<(), Box<dyn Error>> {
    let chunks = ["Hey there!", " How can I help?"];

    let llm_responses = [llm_response("msg_1").text(&chunks).build()];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("hello")],
        })])
        .max_auto_continues(3)
        .run()
        .await?;

    let auto_continue_count =
        messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))).count();
    assert_eq!(auto_continue_count, 0, "Expected no AutoContinue messages for opening message without tool calls");

    assert!(matches!(messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)));

    Ok(())
}

#[tokio::test]
async fn test_auto_continue_triggers_on_length_stop_reason() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(2, 3);
    let llm_responses = [
        llm_response("msg_1").tool_call("call_1", "test__add_numbers", &[&tool_request.json()?]).build(),
        vec![
            LlmResponse::start("msg_2"),
            LlmResponse::text("I'm thinking about the problem..."),
            LlmResponse::done_with_stop_reason(StopReason::Length),
        ],
        vec![
            LlmResponse::start("msg_3"),
            LlmResponse::text("Let me continue..."),
            LlmResponse::done_with_stop_reason(StopReason::Length),
        ],
        vec![
            LlmResponse::start("msg_4"),
            LlmResponse::text("Done!"),
            LlmResponse::done_with_stop_reason(StopReason::EndTurn),
        ],
    ];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .max_auto_continues(5)
        .run()
        .await?;

    let auto_continue_count =
        messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))).count();
    assert_eq!(
        auto_continue_count, 2,
        "Expected 2 AutoContinue messages after length stop reasons, got {auto_continue_count}"
    );

    let auto_continues: Vec<_> = messages
        .iter()
        .filter_map(|m| match m {
            AgentEvent::Turn(TurnEvent::AutoContinue { attempt, max_attempts }) => Some((*attempt, *max_attempts)),
            _ => None,
        })
        .collect();
    assert_eq!(auto_continues, vec![(1, 5), (2, 5)]);

    Ok(())
}

#[tokio::test]
async fn test_auto_continue_triggers_on_empty_length_stop_reason() -> Result<(), Box<dyn Error>> {
    let llm_responses = [
        vec![LlmResponse::start("msg_1"), LlmResponse::done_with_stop_reason(StopReason::Length)],
        vec![
            LlmResponse::start("msg_2"),
            LlmResponse::text("Recovered after compaction"),
            LlmResponse::done_with_stop_reason(StopReason::EndTurn),
        ],
    ];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .max_auto_continues(3)
        .run()
        .await?;

    let auto_continue_count =
        messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))).count();
    assert_eq!(auto_continue_count, 1, "Expected AutoContinue after an empty length stop, got {messages:?}");

    assert!(
        messages.iter().any(|m| matches!(
            m,
            AgentEvent::Message(MessageEvent::Text { chunk, .. }) if chunk == "Recovered after compaction"
        )),
        "Expected follow-up response after empty length stop, got {messages:?}"
    );

    assert!(matches!(messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)));

    Ok(())
}

#[tokio::test]
async fn test_auto_continue_respects_max_limit() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(2, 3);

    let llm_responses = [
        llm_response("msg_1").tool_call("call_1", "test__add_numbers", &[&tool_request.json()?]).build(),
        vec![
            LlmResponse::start("msg_2"),
            LlmResponse::text("Thinking..."),
            LlmResponse::done_with_stop_reason(StopReason::Length),
        ],
        vec![
            LlmResponse::start("msg_3"),
            LlmResponse::text("Still thinking..."),
            LlmResponse::done_with_stop_reason(StopReason::Length),
        ],
        vec![
            LlmResponse::start("msg_4"),
            LlmResponse::text("More thinking..."),
            LlmResponse::done_with_stop_reason(StopReason::Length),
        ],
    ];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .max_auto_continues(2)
        .run()
        .await?;

    let auto_continue_count =
        messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))).count();
    assert_eq!(auto_continue_count, 2, "Expected 2 AutoContinue messages (max limit), got {auto_continue_count}");

    assert!(
        matches!(messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)),
        "Expected the turn to complete after hitting max_auto_continues"
    );

    Ok(())
}

#[tokio::test]
async fn test_auto_continue_disabled_with_zero() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(2, 3);

    let llm_responses = [
        llm_response("msg_1").tool_call("call_1", "test__add_numbers", &[&tool_request.json()?]).build(),
        vec![
            LlmResponse::start("msg_2"),
            LlmResponse::text("No completion signal here"),
            LlmResponse::done_with_stop_reason(StopReason::Length),
        ],
    ];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .max_auto_continues(0)
        .run()
        .await?;

    let auto_continue_count =
        messages.iter().filter(|m| matches!(m, AgentEvent::Turn(TurnEvent::AutoContinue { .. }))).count();
    assert_eq!(auto_continue_count, 0, "Expected no AutoContinue messages when max_auto_continues=0");

    assert!(matches!(messages.last().and_then(AgentEvent::turn_outcome), Some(TurnOutcome::Completed)));

    Ok(())
}

#[tokio::test]
async fn test_reasoning_content_is_saved_in_context_after_tool_call() -> Result<(), Box<dyn Error>> {
    let tool_request = AddNumbersRequest::new(2, 3);

    let llm_responses = [
        vec![
            LlmResponse::start("msg_1"),
            LlmResponse::reasoning("internal plan"),
            LlmResponse::tool_request_start("call_1", "test__add_numbers"),
            LlmResponse::tool_request_arg("call_1", &tool_request.json()?),
            LlmResponse::tool_request_complete("call_1", "test__add_numbers", &tool_request.json()?),
            LlmResponse::done(),
        ],
        llm_response("msg_2").text(&["Done"]).build(),
    ];

    let result = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .run_with_context()
        .await?;

    let contexts = result.captured_contexts.lock().unwrap();
    let second_context = contexts.get(1).expect("expected second LLM request context");

    let assistant_with_tool_call = second_context.messages().iter().find(|message| {
        matches!(
            message,
            ChatMessage::Assistant { tool_calls, .. } if !tool_calls.is_empty()
        )
    });

    let Some(ChatMessage::Assistant { reasoning, .. }) = assistant_with_tool_call else {
        panic!("expected assistant message with tool call");
    };

    assert_eq!(reasoning.summary_text.as_deref(), Some("internal plan"));

    Ok(())
}

#[tokio::test]
async fn test_reasoning_chunks_emit_thought_messages() -> Result<(), Box<dyn Error>> {
    let llm_responses = [vec![
        LlmResponse::start("msg_1"),
        LlmResponse::reasoning("internal plan"),
        LlmResponse::text("Done"),
        LlmResponse::done(),
    ]];

    let messages = test_agent()
        .llm_responses(&llm_responses)
        .user_messages(vec![Command::UserCommand(UserCommand::Text {
            content: vec![llm::ContentBlock::text("do something")],
        })])
        .run()
        .await?;

    assert!(
        messages.iter().any(|m| matches!(
            m,
            AgentEvent::Message(MessageEvent::Thought { chunk, .. }) if chunk == "internal plan"
        )),
        "Expected at least one Thought message from reasoning chunks, got: {messages:?}"
    );
    assert!(
        messages.iter().any(|m| matches!(m, AgentEvent::Turn(TurnEvent::Ended { .. }))),
        "Expected the turn to end"
    );

    Ok(())
}
