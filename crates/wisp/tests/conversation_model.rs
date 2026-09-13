use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use wisp::conversation::{Conversation, ConversationContent, MessageRole, ToolCall, ToolStatus};

fn text(value: &str) -> MaybeUndefined<Vec<acp::ContentBlock>> {
    MaybeUndefined::Value(vec![acp::ContentBlock::Text(acp::TextContent::new(value))])
}

fn tool(conversation: &Conversation) -> &ToolCall {
    let ConversationContent::Tool(tool) = conversation.items()[0].content() else { panic!("tool item") };
    tool
}

#[test]
fn message_ids_isolate_replacement_clearing_and_chunks() {
    let mut conversation = Conversation::new();
    conversation.upsert_message(MessageRole::Assistant, "a".into(), &text("first"));
    conversation.upsert_message(MessageRole::Assistant, "b".into(), &text("second"));
    conversation.append_message_chunk(
        MessageRole::Assistant,
        &acp::ContentChunk::new(acp::ContentBlock::Text(acp::TextContent::new("!")), "a"),
    );
    assert_eq!(conversation.items()[0].text(), Some("first!"));
    assert_eq!(conversation.items()[1].text(), Some("second"));
    let revision = conversation.items()[0].revision();
    conversation.upsert_message(MessageRole::Assistant, "a".into(), &text("replacement"));
    assert!(conversation.items()[0].revision() > revision);
    conversation.upsert_message(MessageRole::Assistant, "a".into(), &MaybeUndefined::Undefined);
    assert_eq!(conversation.items()[0].text(), Some("replacement"));
    conversation.upsert_message(MessageRole::Assistant, "a".into(), &MaybeUndefined::Null);
    assert_eq!(conversation.items()[0].text(), Some(""));
    conversation.upsert_message(MessageRole::Assistant, "b".into(), &MaybeUndefined::Value(vec![]));
    assert_eq!(conversation.items()[1].text(), Some(""));
}

#[test]
fn user_ack_adopts_only_the_explicit_optimistic_item() {
    let mut conversation = Conversation::new();
    conversation.append_user_content("same");
    conversation.append_pending_user_content("same");
    conversation.upsert_message(MessageRole::User, "user".into(), &text("expanded prompt"));
    conversation.upsert_message(MessageRole::User, "user".into(), &text("expanded prompt"));
    assert_eq!(conversation.items().len(), 2);
    assert_eq!(conversation.items()[0].text(), Some("same"));
    assert_eq!(conversation.items()[1].text(), Some("expanded prompt"));
    conversation.clear();
    conversation.upsert_message(MessageRole::User, "user".into(), &text("replayed"));
    assert_eq!(conversation.items().len(), 1);
    assert_eq!(conversation.items()[0].text(), Some("replayed"));
}

#[test]
fn thought_identity_is_distinct_from_assistant_text() {
    let mut conversation = Conversation::new();
    conversation.upsert_message(MessageRole::Thought, "thought".into(), &text("thinking"));
    conversation.upsert_message(MessageRole::Assistant, "answer".into(), &text("answer"));
    conversation.upsert_message(MessageRole::Thought, "thought".into(), &MaybeUndefined::Null);
    assert!(matches!(conversation.items()[0].content(), ConversationContent::Thought(_)));
    assert_eq!(conversation.items()[0].text(), Some(""));
    assert_eq!(conversation.items()[1].text(), Some("answer"));
}

#[test]
fn first_tool_update_creates_and_later_patches_replace_or_clear() {
    let mut conversation = Conversation::new();
    conversation.on_tool_call_update(
        &acp::ToolCallUpdate::new("tool")
            .title("Bash")
            .raw_input(serde_json::json!({"command":"first"}))
            .status(acp::ToolCallStatus::Completed),
    );
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("tool"));
    assert_eq!(tool(&conversation).title, "Bash");
    conversation
        .on_tool_call_update(&acp::ToolCallUpdate::new("tool").raw_input(serde_json::json!({"command":"second"})));
    assert_eq!(tool(&conversation).bash_command().as_deref(), Some("second"));
    conversation.on_tool_call_update(
        &acp::ToolCallUpdate::new("tool").title(MaybeUndefined::Null).raw_input(MaybeUndefined::Null),
    );
    assert_eq!(conversation.items().len(), 1);
    assert!(tool(&conversation).title.is_empty());
    assert!(tool(&conversation).raw_input.is_empty());
}

#[test]
fn tool_content_chunks_append_but_updates_replace() {
    let mut conversation = Conversation::new();
    let content = || acp::ToolCallContent::Content(Box::new(acp::Content::new("output")));
    conversation.on_tool_call_content_chunk(&acp::ToolCallContentChunk::new("tool", content()));
    conversation.on_tool_call_content_chunk(&acp::ToolCallContentChunk::new("tool", content()));
    assert_eq!(tool(&conversation).content().len(), 2);
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("tool").content(vec![content()]));
    assert_eq!(tool(&conversation).content().len(), 1);
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("tool").content(MaybeUndefined::Null));
    assert!(tool(&conversation).content().is_empty());
}

#[test]
fn optimistic_prompt_stays_mutable_until_the_agent_ack() {
    let mut conversation = Conversation::new();
    conversation.append_pending_user_content("prompt");
    assert!(conversation.items()[0].is_open());
    conversation.upsert_message(MessageRole::User, "ack".into(), &text("expanded"));
    assert!(conversation.items()[0].is_open());
    conversation.finish_turn(&ToolStatus::Success);
    assert!(!conversation.items()[0].is_open());
}

#[test]
fn tool_kind_status_and_metadata_follow_patch_semantics() {
    let mut conversation = Conversation::new();
    let update: acp::ToolCallUpdate = serde_json::from_value(serde_json::json!({
        "toolCallId": "tool", "kind": "execute", "rawInput": {"command": "ls"},
        "status": "completed", "_meta": {"display_value": "done"}
    }))
    .unwrap();
    conversation.on_tool_call_update(&update);
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("tool"));
    assert_eq!(tool(&conversation).bash_command(), None);
    assert_eq!(tool(&conversation).display_value.as_deref(), Some("done"));
    assert_eq!(tool(&conversation).status, ToolStatus::Success);
    conversation.on_tool_call_update(
        &acp::ToolCallUpdate::new("tool")
            .kind(MaybeUndefined::Null)
            .status(MaybeUndefined::Null)
            .meta(MaybeUndefined::Null),
    );
    assert_eq!(tool(&conversation).bash_command(), None);
    assert_eq!(tool(&conversation).display_value, None);
    assert_eq!(tool(&conversation).status, ToolStatus::Running);
}

#[test]
fn plan_replacement_and_grace_period_are_scoped_by_plan_id() {
    let mut conversation = Conversation::new();
    let tracker = conversation.plan_tracker_mut();
    let now = Instant::now();
    let entry = |status| acp::PlanEntry::new("same", acp::PlanEntryPriority::Medium, status);
    tracker.replace("a".into(), vec![entry(acp::PlanEntryStatus::Completed)], now);
    tracker.replace("b".into(), vec![entry(acp::PlanEntryStatus::Pending)], now);
    tracker.replace("a".into(), vec![entry(acp::PlanEntryStatus::Other("_aether_cancelled".into()))], now);
    assert_eq!(tracker.visible_entries(now).len(), 2);
    assert_eq!(tracker.visible_entries(now + Duration::from_secs(4)).len(), 1);
    tracker.replace("a".into(), vec![], now);
    assert_eq!(tracker.visible_entries(now)[0].status, acp::PlanEntryStatus::Pending);
    tracker.apply_update(
        &acp::PlanUpdate::new(acp::PlanUpdateContent::Other(acp::OtherPlanUpdateContent::new(
            "future",
            "b",
            BTreeMap::new(),
        ))),
        now,
    );
    assert_eq!(tracker.visible_entries(now).len(), 1);
}
