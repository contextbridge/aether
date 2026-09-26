use agent_client_protocol::schema::v2 as acp;

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
fn patchless_diff_lists_all_changes_without_fabricating_sources() {
    let diff = acp::Diff::new(vec![
        acp::DiffChange::add(path("/workspace/new.rs")),
        acp::DiffChange::delete(path("/workspace/old.rs")),
        acp::DiffChange::modify(path("/workspace/changed.rs")),
        acp::DiffChange::move_file(path("/workspace/from.rs"), path("/workspace/to.rs")),
        acp::DiffChange::copy(path("/workspace/source.rs"), path("/workspace/copy.rs")),
    ]);
    let mut ui = ui_with_diff(diff);
    for label in [
        "A /workspace/new.rs",
        "D /workspace/old.rs",
        "M /workspace/changed.rs",
        "R /workspace/from.rs → /workspace/to.rs",
        "C /workspace/source.rs → /workspace/copy.rs",
    ] {
        ui.assert_conversation_contains(label);
    }
}

#[test]
fn unknown_changes_and_patch_formats_are_displayable_without_parsing() {
    let diff = serde_json::from_value(serde_json::json!({
        "changes": [{"operation": "_future", "path": "/workspace/unknown"}],
        "patch": {"format": "_future", "text": "future patch"}
    }))
    .unwrap();
    let mut ui = ui_with_diff(diff);
    ui.assert_conversation_contains("Unknown file change");
}

#[test]
fn unknown_patch_format_is_not_rendered_as_a_git_patch() {
    let diff = serde_json::from_value(serde_json::json!({
        "changes": [{"operation": "modify", "path": "/workspace/src/lib.rs"}],
        "patch": {"format": "_future", "text": PATCH}
    }))
    .unwrap();
    let mut ui = ui_with_diff(diff);
    ui.assert_conversation_contains("M /workspace/src/lib.rs");
    ui.assert_conversation_not_contains("before");
    ui.assert_conversation_not_contains("after");
}

fn ui_with_diff(diff: acp::Diff) -> wisp::testing::TestUi {
    let mut ui = wisp::testing::TestUi::with_dimensions(100, 30);
    ui.acp_event(wisp::testing::session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new("edit")
            .title("Edit files")
            .status(acp::ToolCallStatus::Completed)
            .content(vec![acp::ToolCallContent::Diff(diff)]),
    )));
    ui
}

fn path(value: &str) -> acp::AbsolutePath {
    acp::AbsolutePath::new(value)
}
