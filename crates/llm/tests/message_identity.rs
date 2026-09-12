use llm::{AssistantReasoning, ChatMessage, Context, MessageId};

#[test]
fn context_serialization_and_projections_preserve_identity() {
    let mut context = Context::new(vec![ChatMessage::system("system"), ChatMessage::user("user")], vec![]);
    let assistant_id = MessageId::new();
    context.push_assistant_turn(assistant_id.clone(), "answer", AssistantReasoning::default(), vec![]);
    let ids = |context: &Context| context.messages().iter().map(ChatMessage::message_id).collect::<Vec<_>>();
    let expected = ids(&context);
    assert_eq!(expected[0], None, "system prompts carry no identity");
    assert_eq!(expected.iter().flatten().collect::<std::collections::HashSet<_>>().len(), 2);
    let json = serde_json::to_string(&context).unwrap();
    let loaded: Context = serde_json::from_str(&json).unwrap();
    for projected in [loaded, context.clone(), context.filter_encrypted_reasoning(None)] {
        assert_eq!(ids(&projected), expected);
    }
    let summary_id = MessageId::new();
    let summary = context.with_compacted_summary(summary_id.clone(), "summary");
    assert_eq!(ids(&summary), vec![None, Some(summary_id.clone())]);
    assert!(!expected.contains(&Some(summary_id)));
    context.clear_conversation();
    assert_eq!(ids(&context), vec![None]);
}
