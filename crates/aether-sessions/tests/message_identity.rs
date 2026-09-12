use aether_core::events::{AgentEvent, Command, MessageEvent, StreamState};
use aether_core::testing::test_agent;
use aether_sessions::testing::{TestStore, assistant_text, compaction_result, user_message};
use aether_sessions::{SessionEvent, UserEvent, context_from_events};
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock, MessageId};

#[tokio::test]
async fn live_context_and_reloaded_log_share_identities_and_message_boundaries() {
    let user_id = MessageId::new();
    let content = vec![ContentBlock::text("calculate")];
    let result = test_agent()
        .llm_responses(&[
            llm_response("reused-provider-id")
                .reasoning(&["plan"])
                .tool_call("call", "test__add_numbers", &["{\"a\":2,\"b\":3}"])
                .build(),
            llm_response("reused-provider-id").text(&["The answer is ", "5"]).build(),
        ])
        .commands(vec![Command::with_message_id(user_id.clone(), content.clone())])
        .run_with_context()
        .await
        .unwrap();

    let mut events = vec![SessionEvent::User(UserEvent::Message { message_id: user_id.clone(), content })];
    events.extend(result.messages.iter().cloned().map(SessionEvent::Agent));
    let store = TestStore::new().session("identity", &events);
    let (_, loaded) = store.store().load("identity").unwrap();
    let restored = context_from_events(&loaded);
    let replayed = context_from_events(&loaded);
    let ids = |messages: &[ChatMessage]| messages.iter().map(ChatMessage::message_id).collect::<Vec<_>>();
    assert_eq!(ids(restored.messages()), ids(replayed.messages()));
    assert_eq!(restored.message_count(), 4);
    assert_eq!(restored.messages()[0].message_id(), user_id);

    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(ids(contexts[1].messages()), ids(&restored.messages()[..3]));
    let assistant_ids: Vec<_> = restored
        .messages()
        .iter()
        .filter_map(|message| match message {
            ChatMessage::Assistant { message_id, .. } => Some(message_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(assistant_ids.len(), 2);
    assert_ne!(assistant_ids[0], assistant_ids[1]);
    assert!(assistant_ids.iter().all(|id| id.as_str() != "reused-provider-id"));
    assert!(
        matches!(&restored.messages()[1], ChatMessage::Assistant { reasoning, .. } if reasoning.summary_text.as_deref() == Some("plan"))
    );

    for event in &result.messages {
        match event {
            AgentEvent::Message(MessageEvent::Text { message_id, .. }) => assert!(assistant_ids.contains(message_id)),
            AgentEvent::Message(MessageEvent::Thought { message_id, .. }) => {
                assert_eq!(*message_id, assistant_ids[0].thought());
            }
            _ => {}
        }
    }
    let mut switched = contexts[1].clone();
    switched.replace_conversation(restored.messages().clone());
    assert_eq!(ids(switched.messages()), ids(restored.messages()));
}

#[tokio::test]
async fn continuation_preserves_live_message_boundaries_and_ids() {
    let user_id = MessageId::new();
    let content = vec![ContentBlock::text("continue")];
    let result = test_agent()
        .llm_responses(&[
            llm_response("provider").text(&["first"]).build_with_stop_reason(llm::StopReason::Length),
            llm_response("provider").text(&["last"]).build(),
        ])
        .max_auto_continues(1)
        .commands(vec![Command::with_message_id(user_id.clone(), content.clone())])
        .run_with_context()
        .await
        .unwrap();
    let contexts = result.captured_contexts.lock().unwrap();
    assert_eq!(contexts[1].message_count(), 3);
    let mut events = vec![SessionEvent::User(UserEvent::Message { message_id: user_id, content })];
    events.extend(result.messages.into_iter().map(SessionEvent::Agent));
    let restored = context_from_events(&events);
    assert_eq!(restored.message_count(), 4);
    for (live, restored) in contexts[1].messages().iter().zip(restored.messages()) {
        assert_eq!(live.message_id(), restored.message_id());
    }
}

#[test]
fn stored_user_and_summary_ids_survive_repeated_reconstruction() {
    let user = user_message("question");
    let summary = compaction_result("summary", 2);
    let store = TestStore::new().session(
        "summary",
        &[user.clone(), assistant_text("answer", "answer"), summary.clone(), user_message("continue")],
    );
    let (_, loaded) = store.store().load("summary").unwrap();
    let user_context = context_from_events(&loaded[..1]);
    let SessionEvent::User(UserEvent::Message { message_id, .. }) = user else { panic!("user") };
    assert_eq!(user_context.messages()[0].message_id(), message_id);
    let SessionEvent::Agent(AgentEvent::Context(aether_core::events::ContextEvent::CompactionResult {
        message_id,
        ..
    })) = summary
    else {
        panic!("summary")
    };
    for _ in 0..2 {
        let context = context_from_events(&loaded);
        assert_eq!(context.messages()[0].message_id(), message_id);
        assert!(context.messages()[0].is_summary());
    }
}

#[test]
fn partial_streams_do_not_replace_durable_messages() {
    let events = [
        assistant_text("original", "complete"),
        SessionEvent::Agent(AgentEvent::text("unfinished", "partial", StreamState::Partial)),
    ];
    let context = context_from_events(&events);
    assert_eq!(context.message_count(), 1);
    assert_eq!(context.messages()[0].message_id().as_str(), "original");
}
