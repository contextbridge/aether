use llm::{AssistantReasoning, ChatMessage, Context, MessageId};

#[test]
fn context_serialization_and_projections_preserve_identity() {
    let mut context = Context::new(vec![ChatMessage::system("system"), ChatMessage::user("user")], vec![]);
    let assistant_id = MessageId::new();
    context.push_assistant_turn(assistant_id.clone(), "answer", AssistantReasoning::default(), vec![]);
    let expected: Vec<_> = context.messages().iter().map(ChatMessage::message_id).collect();
    assert_eq!(expected.iter().collect::<std::collections::HashSet<_>>().len(), 3);
    let json = serde_json::to_string(&context).unwrap();
    let loaded: Context = serde_json::from_str(&json).unwrap();
    for projected in [loaded, context.clone(), context.filter_encrypted_reasoning(None)] {
        assert_eq!(projected.messages().iter().map(ChatMessage::message_id).collect::<Vec<_>>(), expected);
    }
    let summary_id = MessageId::new();
    let summary = context.with_compacted_summary(summary_id.clone(), "summary");
    assert_eq!(summary.messages()[0].message_id(), expected[0]);
    assert_eq!(summary.messages()[1].message_id(), summary_id);
    assert!(!expected.contains(&summary_id));
    context.clear_conversation();
    assert_eq!(context.messages()[0].message_id(), expected[0]);
}
