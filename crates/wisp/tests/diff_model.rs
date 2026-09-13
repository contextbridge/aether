use agent_client_protocol::schema::{MaybeUndefined, v2 as acp};
use wisp::conversation::{Conversation, ConversationContent, ToolCall};

const PATCH: &str = "diff --git a/workspace/src/lib.rs b/workspace/src/lib.rs\n--- a/workspace/src/lib.rs\n+++ b/workspace/src/lib.rs\n@@ -1 +1 @@\n-before\n+after\n";

#[test]
fn text_diff_fixture_uses_v2_metadata_and_preserves_patch_content() {
    let diff = wisp::testing::text_diff("/workspace/file with \"quotes\".rs", "before", "after");
    assert_eq!(diff.changes, vec![acp::DiffChange::modify(path("/workspace/file with \"quotes\".rs"))]);
    let patch = diff.patch.expect("changed content has a patch");
    assert_eq!(patch.format, acp::DiffPatchFormat::GitPatch);
    assert!(patch.text.contains("\\ No newline at end of file"));
    let wire = serde_json::to_value(wisp::testing::text_diff("/workspace/file.rs", "old\n", "new\n")).unwrap();
    assert_eq!(wire["changes"][0]["operation"], "modify");
    assert_eq!(wire["patch"]["format"], "git_patch");
}

#[test]
fn text_diff_fixture_omits_patch_for_unchanged_content() {
    let diff = wisp::testing::text_diff("/workspace/file.rs", "same", "same");
    assert!(diff.patch.is_none());
}

#[test]
fn acp_diff_preserves_patch_and_absolute_change_paths() {
    let conversation =
        conversation_with_diff(acp::Diff::patch(PATCH, vec![acp::DiffChange::modify(path("/workspace/src/lib.rs"))]));
    let diff = &tool(&conversation).diffs()[0];
    assert_eq!(diff.patch.as_deref(), Some(PATCH));
    assert_eq!(diff.changes, ["M /workspace/src/lib.rs"]);
}

#[test]
fn patchless_diff_lists_all_changes_without_fabricating_sources() {
    let conversation = conversation_with_diff(acp::Diff::new(vec![
        acp::DiffChange::add(path("/workspace/new.rs")),
        acp::DiffChange::delete(path("/workspace/old.rs")),
        acp::DiffChange::modify(path("/workspace/changed.rs")),
        acp::DiffChange::move_file(path("/workspace/from.rs"), path("/workspace/to.rs")),
        acp::DiffChange::copy(path("/workspace/source.rs"), path("/workspace/copy.rs")),
    ]));
    let diff = &tool(&conversation).diffs()[0];
    assert!(diff.patch.is_none());
    assert_eq!(
        diff.changes,
        [
            "A /workspace/new.rs",
            "D /workspace/old.rs",
            "M /workspace/changed.rs",
            "R /workspace/from.rs → /workspace/to.rs",
            "C /workspace/source.rs → /workspace/copy.rs"
        ]
    );
}

#[test]
fn unknown_changes_and_patch_formats_are_displayable_without_parsing() {
    let diff = serde_json::from_value(serde_json::json!({
        "changes": [{"operation": "_future", "path": "/workspace/unknown"}],
        "patch": {"format": "_future", "text": "future patch"}
    }))
    .unwrap();
    let conversation = conversation_with_diff(diff);
    let diff = &tool(&conversation).diffs()[0];
    assert_eq!(diff.changes, ["Unknown file change"]);
    assert_eq!(diff.patch.as_deref(), Some("future patch"));
}

#[test]
fn diff_content_replacement_and_null_drop_stale_previews() {
    let mut conversation =
        conversation_with_diff(acp::Diff::patch(PATCH, vec![acp::DiffChange::modify(path("/workspace/a"))]));
    conversation.on_tool_call_content_chunk(&acp::ToolCallContentChunk::new(
        "edit",
        acp::ToolCallContent::Diff(acp::Diff::new(vec![acp::DiffChange::add(path("/workspace/b"))])),
    ));
    assert_eq!(tool(&conversation).diffs().len(), 2);
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("edit"));
    assert_eq!(tool(&conversation).diffs().len(), 2);
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("edit").content(vec![]));
    assert!(tool(&conversation).diffs().is_empty());
    conversation.on_tool_call_content_chunk(&acp::ToolCallContentChunk::new(
        "edit",
        acp::ToolCallContent::Diff(acp::Diff::new(vec![])),
    ));
    conversation.on_tool_call_update(&acp::ToolCallUpdate::new("edit").content(MaybeUndefined::Null));
    assert!(tool(&conversation).diffs().is_empty());
}

fn path(value: &str) -> acp::AbsolutePath {
    acp::AbsolutePath::new(value)
}

fn tool(conversation: &Conversation) -> &ToolCall {
    let ConversationContent::Tool(tool) = conversation.items()[0].content() else { panic!("tool item") };
    tool
}

fn conversation_with_diff(diff: acp::Diff) -> Conversation {
    let mut conversation = Conversation::new();
    conversation.on_tool_call_update(
        &acp::ToolCallUpdate::new("edit").title("Edit files").content(vec![acp::ToolCallContent::Diff(diff)]),
    );
    conversation
}
