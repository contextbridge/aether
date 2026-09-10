use agent_client_protocol::schema::v1 as acp;
use clankerdiff_ratatui::diff::PatchLineKind;
use wisp::conversation::{Conversation, ConversationContent};

#[test]
fn acp_diff_retains_full_sources_and_absolute_path_context() {
    let conversation = conversation_with_diff("/workspace/src/lib.rs");
    let ConversationContent::Tool(tool) = conversation.items()[0].content() else {
        panic!("tool item");
    };
    let file = tool.diff.as_ref().expect("valid preview");
    assert_eq!(file.path.as_str(), "workspace/src/lib.rs");
    assert!(file.old_source.is_ok());
    assert!(file.new_source.is_ok());
    assert!(
        file.hunks
            .iter()
            .flat_map(|hunk| &hunk.lines)
            .any(|line| line.kind == PatchLineKind::Added && line.text.as_ref() == "after")
    );
}

#[test]
fn invalid_acp_diff_path_is_reported_instead_of_panicking() {
    let conversation = conversation_with_diff("../escape.rs");
    let ConversationContent::Tool(tool) = conversation.items()[0].content() else {
        panic!("tool item");
    };
    assert!(tool.diff.is_none());
    assert!(tool.display_value.as_ref().unwrap().contains("Cannot preview ../escape.rs"));
}

fn conversation_with_diff(path: &str) -> Conversation {
    let mut conversation = Conversation::new();
    conversation.on_tool_call(&acp::ToolCall::new("edit", "Edit file"));
    let fields = acp::ToolCallUpdateFields::new()
        .content(vec![acp::ToolCallContent::Diff(acp::Diff::new(path, "after\nkeep\n").old_text("before\nkeep\n"))]);
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("edit", fields));
    conversation
}
