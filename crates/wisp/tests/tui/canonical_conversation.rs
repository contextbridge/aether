use super::support::{ToolStatus, acp};
use wisp::conversation::{Conversation, ConversationContent, ItemState};

#[test]
fn streaming_chunks_coalesce_and_revisions_advance() {
    let mut conversation = Conversation::new();

    conversation.append_assistant_chunk("Hel");
    let first_revision = conversation.items()[0].revision();
    conversation.append_assistant_chunk("lo");

    assert_eq!(conversation.items().len(), 1);
    assert_eq!(conversation.items()[0].text(), Some("Hello"));
    assert_eq!(conversation.items()[0].state(), ItemState::Open);
    assert!(conversation.items()[0].revision() > first_revision);
}

#[test]
fn assistant_content_is_sealed_by_the_end_of_the_turn() {
    let mut conversation = Conversation::new();

    conversation.append_assistant_chunk("answer");
    assert_eq!(conversation.items()[0].state(), ItemState::Open);

    conversation.finish_turn(&ToolStatus::Success);

    assert!(matches!(conversation.items()[0].content(), ConversationContent::Assistant(_)));
    assert!(conversation.items().iter().all(|item| item.state() == ItemState::Sealed));
}

#[test]
fn tool_updates_are_owned_and_duplicate_starts_do_not_duplicate_items() {
    let mut conversation = Conversation::new();
    let mut tool = acp::ToolCall::new("tool-1".to_string(), "Read file");
    tool.raw_input = Some(serde_json::json!({"path": "src/lib.rs"}));

    conversation.on_tool_call(&tool);
    conversation.on_tool_call(&tool);
    conversation
        .on_tool_call_update(&acp::ToolCallUpdate::new("tool-1".to_string(), acp::ToolCallUpdateFields::default()));

    assert_eq!(conversation.items().len(), 1);
    assert!(matches!(conversation.items()[0].content(), ConversationContent::Tool(_)));
    assert_eq!(conversation.items()[0].state(), ItemState::Open);

    conversation.finish_turn(&ToolStatus::Success);
    assert_eq!(conversation.items()[0].state(), ItemState::Sealed);
}

#[test]
fn a_terminal_status_seals_the_tool_item_mid_turn() {
    let mut conversation = Conversation::new();
    conversation.on_tool_call(&acp::ToolCall::new("tool-1".to_string(), "Read file"));
    assert_eq!(conversation.items()[0].state(), ItemState::Open);

    conversation.on_tool_call_update(&acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    ));

    assert_eq!(conversation.items()[0].state(), ItemState::Sealed);
}

#[test]
fn clear_replaces_identity_and_resets_items_atomically() {
    let mut conversation = Conversation::new();
    let previous_id = conversation.id();
    conversation.append_user_content("before");

    conversation.clear();

    assert_ne!(conversation.id(), previous_id);
    assert!(conversation.items().is_empty());
}

#[test]
fn notices_are_semantically_distinct_from_user_content() {
    let mut conversation = Conversation::new();

    conversation.append_user_content("prompt");
    conversation.append_notice("Context cleared");

    assert!(matches!(conversation.items()[0].content(), ConversationContent::User(_)));
    assert!(matches!(conversation.items()[1].content(), ConversationContent::Notice(_)));
}
