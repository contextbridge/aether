use aether_core::events::{AgentEvent, Command, MessageEvent, StreamState};
use aether_core::testing::test_agent;
use aether_sessions::testing::{TestStore, assistant_reply, compaction_result, user_message};
use aether_sessions::{SessionEvent, UserEvent, context_from_events};
use llm::testing::llm_response;
use llm::{ChatMessage, ContentBlock, MessageId};

#[tokio::test]
async fn live_context_and_reloaded_log_share_identities_and_message_boundaries() {
    let user_id = MessageId::new();
    let content = vec![ContentBlock::text("calculate")];
    let result = test_agent()
        .llm_responses(&[
            llm_response().reasoning(&["plan"]).tool_call("call", "test__add_numbers", &["{\"a\":2,\"b\":3}"]).build(),
            llm_response().text(&["The answer is ", "5"]).build(),
        ])
        .commands(vec![Command::with_message_id(user_id.clone(), content.clone())])
        .run_with_context()
        .await
        .unwrap();

    let mut events =
        vec![SessionEvent::User(UserEvent::Message { message_id: user_id.clone(), content, display_content: None })];
    events.extend(result.messages.iter().cloned().map(SessionEvent::Agent));
    let store = TestStore::new().session("identity", &events);
    let (_, loaded) = store.store().load("identity").unwrap();
    let restored = context_from_events(&loaded);
    let replayed = context_from_events(&loaded);
    assert_eq!(conversation_ids(restored.messages()), conversation_ids(replayed.messages()));
    assert_eq!(restored.message_count(), 4);
    assert_eq!(restored.messages()[0].message_id(), Some(user_id));

    let contexts = &result.contexts;
    assert_eq!(conversation_ids(contexts[1].messages()), conversation_ids(&restored.messages()[..3]));
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
    assert!(matches!(&restored.messages()[1], ChatMessage::Assistant { content, .. } if content.reasoning() == "plan"));
    assert!(contexts[1].messages().contains(&restored.messages()[1]), "the reply is restored exactly as it was sent");

    for event in &result.messages {
        match event {
            AgentEvent::Message(MessageEvent::Text { message_id, .. }) => assert!(assistant_ids.contains(message_id)),
            AgentEvent::Message(MessageEvent::Thought { message_id, .. }) => {
                assert_eq!(*message_id, assistant_ids[0]);
            }
            _ => {}
        }
    }
    let mut switched = contexts[1].clone();
    switched.replace_conversation(restored.messages().clone());
    assert_eq!(conversation_ids(switched.messages()), conversation_ids(restored.messages()));
}

#[tokio::test]
async fn continuation_preserves_live_message_boundaries_and_ids() {
    let user_id = MessageId::new();
    let content = vec![ContentBlock::text("continue")];
    let result = test_agent()
        .llm_responses(&[
            llm_response().text(&["first"]).build_with_stop_reason(llm::StopReason::Length),
            llm_response().text(&["last"]).build(),
        ])
        .max_auto_continues(1)
        .commands(vec![Command::with_message_id(user_id.clone(), content.clone())])
        .run_with_context()
        .await
        .unwrap();
    let contexts = result.contexts;
    let mut events =
        vec![SessionEvent::User(UserEvent::Message { message_id: user_id, content, display_content: None })];
    events.extend(result.messages.into_iter().map(SessionEvent::Agent));
    let restored = context_from_events(&events);
    assert_eq!(restored.message_count(), 4);
    assert_eq!(conversation_ids(contexts[1].messages()), conversation_ids(&restored.messages()[..3]));
}

#[test]
fn stored_user_and_summary_ids_survive_repeated_reconstruction() {
    let user = user_message("question");
    let summary = compaction_result("summary", 2);
    let store = TestStore::new().session(
        "summary",
        &[user.clone(), assistant_reply("answer", "answer"), summary.clone(), user_message("continue")],
    );
    let (_, loaded) = store.store().load("summary").unwrap();
    assert_eq!(loaded[2], summary);
    let wire = serde_json::to_value(&summary).unwrap();
    assert_eq!(wire["data"]["event"]["compaction_id"], "compaction");
    let user_context = context_from_events(&loaded[..1]);
    let SessionEvent::User(UserEvent::Message { message_id, .. }) = user else { panic!("user") };
    assert_eq!(user_context.messages()[0].message_id(), Some(message_id));
    let SessionEvent::Agent(AgentEvent::Context(aether_core::events::ContextEvent::CompactionResult {
        message_id,
        ..
    })) = summary
    else {
        panic!("summary")
    };
    for _ in 0..2 {
        let context = context_from_events(&loaded);
        assert_eq!(context.messages()[0].message_id(), Some(message_id.clone()));
        assert!(context.messages()[0].is_summary());
    }
}

#[test]
fn partial_streams_do_not_replace_durable_messages() {
    let events = [
        assistant_reply("original", "complete"),
        SessionEvent::Agent(AgentEvent::text("unfinished", "partial", StreamState::Partial)),
    ];
    let context = context_from_events(&events);
    assert_eq!(context.message_count(), 1);
    assert_eq!(context.messages()[0].message_id().as_deref(), Some("original"));
}

fn conversation_ids(messages: &[ChatMessage]) -> Vec<Option<MessageId>> {
    messages.iter().filter(|message| !message.is_system()).map(ChatMessage::message_id).collect()
}
