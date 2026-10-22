use crate::support::{replying, start_thread};
use acp_utils::conversation::ConversationItem;
use acp_utils::notifications::{SubAgentEvent, SubAgentProgressParams};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use aether_desktop::{DesktopTheme, ThemePair};
use agent_client_protocol::schema::v2 as acp;
use agent_client_protocol::{Client, ConnectionTo, JsonRpcNotification};
use clankerdiff_core::git_patch_from_texts;
use clankerdiff_gpui::style::color;
use gpui::{Entity, TestAppContext, WindowAppearance, px};

#[gpui::test]
fn a_code_block_in_a_reply_copies_its_code(cx: &mut TestAppContext) {
    let reply = "Run this:\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n";
    let harness = harness_with(cx, replying("alpha-session", reply));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    send(&harness, cx, &thread, "how?");
    harness.click(cx, "copy-code-1-0").unwrap();

    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("fn main() {\n    println!(\"hi\");\n}"));
}

#[gpui::test]
fn each_code_block_has_its_own_copy_button(cx: &mut TestAppContext) {
    let reply = "```\nfirst\n```\n\nand\n\n```sh\nsecond\n```\n";
    let harness = harness_with(cx, replying("alpha-session", reply));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    send(&harness, cx, &thread, "two blocks");
    harness.click(cx, "copy-code-1-1").unwrap();

    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("second"));
}

#[gpui::test]
fn bash_commands_have_syntax_colors_but_their_output_does_not(cx: &mut TestAppContext) {
    let command = "printf '%s\\n' \"$HOME\"";
    let output = "echo '$HOME is output, not code'";
    let tool = acp::ToolCallUpdate::new("shell-1")
        .name("coding__bash")
        .title("Run command")
        .raw_input(Some(serde_json::json!({"command": command, "description": "Show home"})))
        .status(acp::ToolCallStatus::Completed)
        .content(vec![output.into()]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(tool)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "run it");

    let summary = format!("$ {command}");
    let painted = harness.painted_text(cx, &summary).expect("the command summary paints");
    let string = summary.find("'%s").unwrap();
    assert_ne!(painted.color_at(string), painted.foreground, "Bash strings have syntax colors");

    harness.click(cx, "tool-card-1").unwrap();
    let painted = harness.painted_text(cx, command).expect("expanded details show the actual command");
    assert_ne!(painted.color_at(command.find("'%s").unwrap()), painted.foreground);
    let painted = harness.painted_text(cx, output).expect("stdout paints unchanged");
    assert!(painted.highlights.is_empty(), "stdout is not Bash source");
}

#[gpui::test]
fn inline_tool_diffs_highlight_source_not_just_added_and_removed_lines(cx: &mut TestAppContext) {
    let old = "let greeting = \"old\";\n";
    let new = "let greeting = \"héllo 🦀\";\n";
    let patch = git_patch_from_texts("/alpha/src/lib.rs", Some(old), Some(new)).unwrap().unwrap();
    let diff = acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/src/lib.rs")]);
    let tool = acp::ToolCallUpdate::new("edit-1")
        .title("Edit file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![diff.into()]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(tool)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "edit it");
    harness.click(cx, "tool-card-1").unwrap();

    let source = new.trim_end();
    let painted = harness.painted_text(cx, source).expect("the diff paints source separately from its gutter");
    assert_ne!(
        painted.color_at(source.find('"').unwrap()),
        painted.color_at(source.find("greeting").unwrap()),
        "strings and identifiers have different syntax colors on the same added line"
    );
    assert_eq!(painted.foreground, color(harness.theme(cx).review().diff.foreground));
}

#[gpui::test]
fn multiline_bash_preserves_unicode_and_heredoc_context(cx: &mut TestAppContext) {
    let command = "cat <<'EOF'\nhéllo 🦀 $HOME\nEOF\nprintf '%s\\n' \"$USER\"";
    let harness = expanded_tool(cx, shell(command));
    let painted = harness.painted_text(cx, command).expect("the entire command paints unchanged");
    assert_ne!(painted.color_at(command.find("héllo").unwrap()), painted.foreground);
    assert_ne!(painted.color_at(command.find("'%s").unwrap()), painted.foreground);
    for (range, _) in &painted.highlights {
        assert!(command.is_char_boundary(range.start) && command.is_char_boundary(range.end));
    }
}

#[gpui::test]
fn failed_bash_keeps_the_error_summary_and_highlights_the_expanded_command(cx: &mut TestAppContext) {
    let command = "printf '%s' \"$MISSING\"";
    let tool = shell(command).status(acp::ToolCallStatus::Failed).content(vec!["permission denied".into()]);
    let harness = expanded_tool(cx, tool);
    let error = harness.painted_text(cx, "permission denied").expect("the failure is visible");
    assert!(error.highlights.is_empty(), "errors remain plain text");
    let command_text = harness.painted_text(cx, command).expect("the command is still available");
    assert_ne!(command_text.color_at(command.find("'%s").unwrap()), command_text.foreground);
}

#[gpui::test]
fn inline_diffs_keep_old_and_new_multiline_syntax_context(cx: &mut TestAppContext) {
    let old = "/*\nold comment\n*/\nlet value = 1;\n";
    let new = "/*\nnew comment\n*/\nlet value = 2;\n";
    let patch = git_patch_from_texts("/alpha/src/lib.rs", Some(old), Some(new)).unwrap().unwrap();
    let harness = expanded_tool(cx, edit(acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/src/lib.rs")])));
    let expected = color(harness.theme(cx).review().syntax.style("comment").unwrap().foreground);
    for text in ["old comment", "new comment"] {
        let painted = harness.painted_text(cx, text).expect("both sides paint");
        assert_eq!(painted.color_at(0), expected, "{text} stays a comment across patch lines");
    }
}

#[gpui::test]
fn a_patch_can_highlight_multiple_file_languages(cx: &mut TestAppContext) {
    let rust = "let count = 42;\n";
    let python = "print(\"héllo\")\n";
    let patch = [
        git_patch_from_texts("/alpha/a.rs", None, Some(rust)).unwrap().unwrap(),
        git_patch_from_texts("/alpha/b.py", None, Some(python)).unwrap().unwrap(),
    ]
    .concat();
    let harness = expanded_tool(
        cx,
        edit(acp::Diff::patch(patch, vec![acp::DiffChange::add("/alpha/a.rs"), acp::DiffChange::add("/alpha/b.py")])),
    );
    let rust = harness.painted_text(cx, rust.trim_end()).unwrap();
    assert_ne!(rust.color_at(rust.text.find("42").unwrap()), rust.foreground);
    let python = harness.painted_text(cx, python.trim_end()).unwrap();
    assert_ne!(python.color_at(python.text.find('"').unwrap()), python.foreground);
    assert!(harness.painted_text(cx, "alpha/a.rs").is_some());
    assert!(harness.painted_text(cx, "alpha/b.py").is_some());
}

#[gpui::test]
fn malformed_and_unknown_patch_formats_remain_readable(cx: &mut TestAppContext) {
    let patch = "diff --git a/file.rs b/file.rs\n--- a/file.rs\n+++ b/file.rs\n@@ invalid @@\n+let value = 1;\n";
    let mut diff = acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/file.rs")]);
    let harness = expanded_tool(cx, edit(diff.clone()));
    assert!(harness.painted_text(cx, patch).is_some(), "malformed source is not silently dropped");

    diff.patch.as_mut().unwrap().format = acp::DiffPatchFormat::Other("_custom".into());
    let harness = expanded_tool(cx, edit(diff));
    let painted = harness.painted_text(cx, patch).expect("an unknown format is shown verbatim");
    assert!(painted.highlights.is_empty(), "unknown patch formats are not parsed as code");
}

#[gpui::test]
fn unrecognized_git_patch_text_is_not_silently_hidden(cx: &mut TestAppContext) {
    let patch = "The agent supplied a summary instead of a Git patch.";
    let diff = acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/file.rs")]);
    let harness = expanded_tool(cx, edit(diff));
    assert!(harness.painted_text(cx, patch).is_some());
}

#[gpui::test]
fn painted_text_observations_are_scoped_to_their_window(cx: &mut TestAppContext) {
    let command = "echo 'first window only'";
    let first = expanded_tool(cx, shell(command));
    assert!(first.painted_text(cx, command).is_some());

    let second = expanded_tool(cx, shell("echo 'second window'"));
    assert!(second.painted_text(cx, command).is_none());
}

#[gpui::test]
fn binary_and_patchless_changes_keep_their_file_summaries(cx: &mut TestAppContext) {
    let diff = acp::Diff::new(vec![
        acp::DiffChange::add("/alpha/image.png").file_type(acp::DiffFileType::Binary),
        acp::DiffChange::delete("/alpha/old.rs"),
    ]);
    let harness = expanded_tool(cx, edit(diff));
    assert!(harness.bounds(cx, "tool-changes-1").is_some());
    assert!(harness.bounds(cx, "tool-change-summary-1").is_some());
    assert!(harness.painted_text(cx, "A /alpha/image.png").is_some());
    assert!(harness.painted_text(cx, "D /alpha/old.rs").is_some());
}

#[gpui::test]
fn an_empty_patch_still_shows_its_changed_paths(cx: &mut TestAppContext) {
    let diff = acp::Diff::patch("", vec![acp::DiffChange::modify("/alpha/empty.rs")]);
    let harness = expanded_tool(cx, edit(diff));
    assert!(harness.painted_text(cx, "M /alpha/empty.rs").is_some());
}

#[gpui::test]
fn unknown_file_languages_remain_plain_source(cx: &mut TestAppContext) {
    let source = "arbitrary héllo 🦀 content";
    let patch = git_patch_from_texts("/alpha/file.unknown", None, Some(source)).unwrap().unwrap();
    let harness = expanded_tool(cx, edit(acp::Diff::patch(patch, vec![acp::DiffChange::add("/alpha/file.unknown")])));
    let painted = harness.painted_text(cx, source).expect("unknown-language source stays readable");
    assert!(painted.highlights.is_empty(), "unknown file languages remain plain text");
}

#[gpui::test]
fn renamed_files_use_each_sides_language(cx: &mut TestAppContext) {
    let patch = "diff --git a/old.rs b/new.py\nsimilarity index 50%\nrename from old.rs\nrename to new.py\n--- a/old.rs\n+++ b/new.py\n@@ -1 +1 @@\n-let value = \"old\";\n+print(\"new\")\n";
    let harness = expanded_tool(cx, edit(acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/new.py")])));
    assert!(harness.painted_text(cx, "old.rs → new.py").is_some());
    let old = harness.painted_text(cx, "let value = \"old\";").unwrap();
    assert_ne!(old.color_at(0), old.foreground, "the old side still uses Rust syntax");
    let new = harness.painted_text(cx, "print(\"new\")").unwrap();
    assert_ne!(new.color_at(new.text.find('"').unwrap()), new.foreground);
}

#[gpui::test]
fn code_fences_keep_syntax_colors_after_sharing_the_tool_renderer(cx: &mut TestAppContext) {
    let source = "let text = \"héllo 🦀\";\n// a comment";
    let harness = harness_with(cx, replying("alpha-session", &format!("```rust\n{source}\n```")));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "show code");
    let painted = harness.painted_text(cx, source).unwrap();
    assert_ne!(painted.color_at(source.find('"').unwrap()), painted.foreground);
    assert_eq!(
        painted.color_at(source.find("//").unwrap()),
        color(harness.theme(cx).review().syntax.style("comment").unwrap().foreground)
    );
}

#[gpui::test]
fn commands_and_cached_diffs_recolor_when_the_theme_changes(cx: &mut TestAppContext) {
    let command = "printf '%s' \"héllo\"";
    let source = "let text = \"hello\";";
    let patch = git_patch_from_texts("/alpha/a.rs", None, Some(source)).unwrap().unwrap();
    let tool = shell(command).content(vec![acp::Diff::patch(patch, vec![acp::DiffChange::add("/alpha/a.rs")]).into()]);
    let harness = expanded_tool(cx, tool);
    let light_command = harness.painted_text(cx, command).unwrap();
    let light_diff = harness.painted_text(cx, source).unwrap();

    cx.update(|cx| cx.set_global(DesktopTheme::new(ThemePair::DEFAULT, WindowAppearance::Dark)));

    let dark_command = harness.painted_text(cx, command).unwrap();
    let dark_diff = harness.painted_text(cx, source).unwrap();
    assert_ne!(
        dark_command.color_at(command.find("'%s").unwrap()),
        light_command.color_at(command.find("'%s").unwrap())
    );
    assert_ne!(dark_diff.color_at(source.find('"').unwrap()), light_diff.color_at(source.find('"').unwrap()));
    assert_eq!(dark_diff.foreground, color(harness.theme(cx).review().diff.foreground));
}

#[gpui::test]
fn recent_sub_agent_commands_are_syntax_highlighted(cx: &mut TestAppContext) {
    let (harness, thread, connection) = live_transcript(cx);
    notify(
        &harness,
        cx,
        &thread,
        &connection,
        acp::UpdateSessionNotification::new(
            "alpha-session",
            acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new("parent").name("spawn_subagent").title("Explore"),
            ),
        ),
    );
    let command = "echo 'sub-agent source'";
    notify(
        &harness,
        cx,
        &thread,
        &connection,
        SubAgentProgressParams {
            session_id: "alpha-session".into(),
            parent_tool_id: "parent".into(),
            task_id: "child".into(),
            agent_name: "Explorer".into(),
            event: SubAgentEvent::ToolCallUpdate(Box::new(shell(command))),
        },
    );
    let summary = format!("$ {command}");
    let painted =
        harness.painted_text(cx, &summary).expect("recent sub-agent commands paint while the parent is folded");
    assert_ne!(painted.color_at(summary.find('\'').unwrap()), painted.foreground);
}

#[gpui::test]
fn updated_tool_patches_replace_the_cached_preview(cx: &mut TestAppContext) {
    let (harness, thread, connection) = live_transcript(cx);
    let tool = |source: &str| {
        edit(acp::Diff::patch(
            git_patch_from_texts("/alpha/a.rs", None, Some(source)).unwrap().unwrap(),
            vec![acp::DiffChange::add("/alpha/a.rs")],
        ))
    };
    notify(
        &harness,
        cx,
        &thread,
        &connection,
        acp::UpdateSessionNotification::new(
            "alpha-session",
            acp::SessionUpdate::ToolCallUpdate(tool("let value = 1;")),
        ),
    );
    harness.click(cx, "tool-card-0").unwrap();
    assert!(harness.painted_text(cx, "let value = 1;").is_some());

    let source = "let value = \"updated\";";
    notify(
        &harness,
        cx,
        &thread,
        &connection,
        acp::UpdateSessionNotification::new("alpha-session", acp::SessionUpdate::ToolCallUpdate(tool(source))),
    );

    let painted = harness.painted_text(cx, source).expect("the new patch replaces the old content");
    assert_ne!(painted.color_at(source.find('"').unwrap()), painted.foreground);
}

#[gpui::test]
fn a_tool_card_unfolds_its_input_and_output(cx: &mut TestAppContext) {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from("file contents")]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(tool)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "read it");
    assert!(harness.bounds(cx, "tool-card-1").is_some(), "the tool call follows the prompt");
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "a tool card starts folded");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_some(), "clicking unfolds it");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "clicking again folds it");
}

#[gpui::test]
fn a_tool_disclosure_reserves_a_trailing_slot_without_shifting_on_hover(cx: &mut TestAppContext) {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from("file contents")]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(tool)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "read it");
    harness.hover(cx, "sidebar").unwrap();

    let row = harness.bounds(cx, "tool-card-1").unwrap();
    let slot = harness.bounds(cx, "tool-disclosure-1").expect("the disclosure has a reserved slot");
    let title = harness.bounds(cx, "tool-title-1").unwrap();
    assert!(slot.origin.x > row.center().x, "the disclosure is on the right, not the leading edge");
    assert!(slot.right() <= row.right());

    harness.hover(cx, "tool-card-1").unwrap();
    assert_eq!(harness.bounds(cx, "tool-disclosure-1").unwrap(), slot);
    assert_eq!(harness.bounds(cx, "tool-title-1").unwrap(), title, "hovering does not shift the label");

    harness.click(cx, "tool-card-1").unwrap();
    harness.hover(cx, "sidebar").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_some());
    let expanded_slot = harness.bounds(cx, "tool-disclosure-1").unwrap();
    assert_eq!(expanded_slot.origin.x, slot.origin.x);
    assert_eq!(expanded_slot.size, slot.size);
}

#[gpui::test]
fn a_focused_tool_row_can_be_toggled_with_the_keyboard(cx: &mut TestAppContext) {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from("file contents")]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(tool)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "read it");
    harness.click(cx, "tool-card-1").unwrap();
    harness.hover(cx, "sidebar").unwrap();

    harness.press_key(cx, "space");
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "Space folds the focused row");

    harness.press_key(cx, "enter");
    assert!(harness.bounds(cx, "tool-details-1").is_some(), "Enter unfolds the focused row");
}

#[gpui::test]
fn a_tool_row_summarizes_changes_and_only_unfolds_the_patch_on_demand(cx: &mut TestAppContext) {
    let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let diff = acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/src/lib.rs")]);
    let edit = acp::ToolCallUpdate::new("call-1")
        .title("Edit file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from(diff)]);
    let read = acp::ToolCallUpdate::new("call-2").title("Read file").status(acp::ToolCallStatus::Completed);
    let updates = vec![acp::SessionUpdate::ToolCallUpdate(edit), acp::SessionUpdate::ToolCallUpdate(read)];
    let harness = harness_with(cx, updating("alpha-session", updates));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    send(&harness, cx, &thread, "edit it");

    assert!(harness.bounds(cx, "tool-change-summary-1").is_some(), "the edit summarizes its changes while folded");
    assert!(harness.bounds(cx, "tool-changes-1").is_none(), "patches start folded");
    assert!(harness.bounds(cx, "tool-change-summary-2").is_none(), "a read has no change summary");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-changes-1").is_some(), "expanding reveals the patch");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-changes-1").is_none(), "folding hides the patch again");
}

#[gpui::test]
fn consecutive_tool_rows_are_compact_and_scale_with_the_interface(cx: &mut TestAppContext) {
    let updates = (1..=2)
        .map(|id| {
            acp::SessionUpdate::ToolCallUpdate(
                acp::ToolCallUpdate::new(format!("call-{id}"))
                    .title("Read file")
                    .status(acp::ToolCallStatus::Completed),
            )
        })
        .collect();
    let harness = harness_with(cx, updating("alpha-session", updates));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "read both");

    for _ in 0..2 {
        let row = harness.bounds(cx, "tool-card-1").unwrap();
        let next = harness.bounds(cx, "tool-card-2").unwrap();
        let scale = harness.rem_size(cx) / px(16.0);
        assert!(
            (row.size.height - px(28.0 * scale)).abs() <= px(0.5),
            "the row scales to 28px, snapped to a physical pixel"
        );
        assert_eq!(next.origin.y - row.bottom(), px(0.0), "consecutive calls form one activity flow");
        harness.simulate_keystrokes(cx, "secondary-=");
    }
}

#[gpui::test]
fn long_tool_titles_leave_change_counts_inside_a_narrow_transcript(cx: &mut TestAppContext) {
    let diff = acp::Diff::patch(
        "+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n",
        vec![acp::DiffChange::modify("/alpha/src/lib.rs")],
    );
    let edit = acp::ToolCallUpdate::new("call-1")
        .title("Update a very long file name and verify the changes throughout the entire workspace".repeat(4))
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from(diff)]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(edit)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    harness.resize(cx, 640.0, 600.0);
    send(&harness, cx, &thread, "edit it");

    let row = harness.bounds(cx, "tool-card-1").unwrap();
    let summary = harness.bounds(cx, "tool-change-summary-1").unwrap();
    let transcript = harness.bounds(cx, "transcript").unwrap();
    assert!(row.right() <= transcript.right(), "the tool row fits the transcript");
    assert!(summary.right() <= row.right(), "change counts are not pushed out by the title");
    assert!(summary.size.width > px(0.0), "change counts stay visible");
    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-changes-1").is_some());
}

#[gpui::test]
fn an_expanded_tool_row_shows_the_entire_patch(cx: &mut TestAppContext) {
    let additions = "new line\n".repeat(24);
    let patch = git_patch_from_texts("/alpha/src/lib.rs", None, Some(&additions)).unwrap().unwrap();
    let diff = acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/src/lib.rs")]);
    let edit = acp::ToolCallUpdate::new("call-1")
        .title("Edit file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from(diff)]);
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(edit)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "edit it");

    harness.click(cx, "tool-card-1").unwrap();

    let changes = harness.bounds(cx, "tool-changes-1").expect("the patch is visible");
    let scale = harness.rem_size(cx) / px(16.0);
    assert!(changes.size.height >= px(26.0 * 16.0 * scale), "all patch lines are shown");
}

#[gpui::test]
fn reasoning_unfolds_to_show_its_text(cx: &mut TestAppContext) {
    let updates = vec![
        acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new("pondering".into(), "thought-0".to_owned())),
        acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new("done".into(), "message-0".to_owned())),
    ];
    let harness = harness_with(cx, updating("alpha-session", updates));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "think");
    assert!(harness.bounds(cx, "thinking-text-1").is_none(), "reasoning starts folded");

    harness.click(cx, "thinking-1").unwrap();

    assert!(harness.bounds(cx, "thinking-text-1").is_some());
}

#[gpui::test]
fn the_plan_stays_in_view_while_it_has_work_left(cx: &mut TestAppContext) {
    let plan = plan(&[("write tests", acp::PlanEntryStatus::Completed), ("fix bug", acp::PlanEntryStatus::InProgress)]);
    let harness = harness_with(cx, updating("alpha-session", vec![plan]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    assert!(harness.bounds(cx, "plan-panel").is_none(), "there is no plan yet");

    send(&harness, cx, &thread, "plan it");

    assert!(harness.bounds(cx, "plan-entry-0").is_some());
    assert!(harness.bounds(cx, "plan-entry-1").is_some());
}

#[gpui::test]
fn the_plan_folds_down_to_its_header(cx: &mut TestAppContext) {
    let plan = plan(&[("fix bug", acp::PlanEntryStatus::Pending)]);
    let harness = harness_with(cx, updating("alpha-session", vec![plan]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "plan it");

    harness.click(cx, "plan-header").unwrap();

    assert!(harness.bounds(cx, "plan-header").is_some());
    assert!(harness.bounds(cx, "plan-entry-0").is_none());
}

#[gpui::test]
fn a_finished_plan_hides_once_the_turn_ends(cx: &mut TestAppContext) {
    let plan = plan(&[("fix bug", acp::PlanEntryStatus::Completed)]);
    let harness = harness_with(cx, updating("alpha-session", vec![plan]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");

    send(&harness, cx, &thread, "plan it");

    assert!(harness.bounds(cx, "plan-panel").is_none());
}

#[gpui::test]
fn scrolling_up_offers_a_way_back_to_the_latest_reply(cx: &mut TestAppContext) {
    let reply = (0..200).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n\n");
    let connector = FakeConnector::default().agent("/alpha", replying("alpha-session", &reply));
    let harness =
        DesktopHarnessBuilder::default().connector(connector).project("/alpha").window_size(900.0, 600.0).build(cx);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "talk a lot");
    assert!(harness.bounds(cx, "jump-to-latest").is_none(), "the transcript follows the reply");

    harness.scroll(cx, "transcript", 10.0).unwrap();
    assert!(harness.bounds(cx, "jump-to-latest").is_some(), "scrolling up stops following");

    harness.click(cx, "jump-to-latest").unwrap();
    assert!(harness.bounds(cx, "jump-to-latest").is_none(), "jumping back follows again");
}

/// An agent that opens `session_id` and answers every prompt with `updates`.
fn updating(session_id: &str, updates: Vec<acp::SessionUpdate>) -> FakeAgent {
    FakeAgent::default().new_session_response(acp::NewSessionResponse::new(session_id)).prompt_updates(updates)
}

fn plan(entries: &[(&str, acp::PlanEntryStatus)]) -> acp::SessionUpdate {
    let entries = entries
        .iter()
        .map(|(content, status)| acp::PlanEntry::new(*content, acp::PlanEntryPriority::Medium, status.clone()))
        .collect();
    acp::SessionUpdate::PlanUpdate(acp::PlanUpdate::new(acp::PlanUpdateContent::items("plan", entries)))
}

fn harness_with(cx: &mut TestAppContext, agent: FakeAgent) -> DesktopHarness {
    let connector = FakeConnector::default().agent("/alpha", agent);
    DesktopHarnessBuilder::default().connector(connector).project("/alpha").build(cx)
}

/// Sends `prompt` through the composer and waits for the agent's turn on it to end.
fn send(harness: &DesktopHarness, cx: &mut TestAppContext, thread: &Entity<Thread>, prompt: &str) {
    harness.simulate_input(cx, prompt);
    harness.simulate_keystrokes(cx, "enter");
    harness.run_until(cx, |cx| {
        let thread = thread.read(cx);
        thread.status() == ThreadStatus::Idle
            && thread.core().is_some_and(|core| !core.conversation().items().is_empty())
    });
}

fn shell(command: &str) -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new("shell-1")
        .name("coding__bash")
        .title("Run command")
        .raw_input(Some(serde_json::json!({"command": command})))
        .status(acp::ToolCallStatus::Completed)
}

fn edit(diff: acp::Diff) -> acp::ToolCallUpdate {
    acp::ToolCallUpdate::new("edit-1")
        .title("Edit file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![diff.into()])
}

fn expanded_tool(cx: &mut TestAppContext, tool: acp::ToolCallUpdate) -> DesktopHarness {
    let harness = harness_with(cx, updating("alpha-session", vec![acp::SessionUpdate::ToolCallUpdate(tool)]));
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    send(&harness, cx, &thread, "show tool");
    harness.click(cx, "tool-card-1").unwrap();
    harness
}

fn live_transcript(cx: &mut TestAppContext) -> (DesktopHarness, Entity<Thread>, ConnectionTo<Client>) {
    let (agent, mut requests) =
        FakeAgent::default().new_session_response(acp::NewSessionResponse::new("alpha-session")).capture();
    let harness = harness_with(cx, agent);
    let thread = start_thread(&harness, cx, "project-alpha-new-thread");
    let connection = requests.connection.try_recv().unwrap();
    (harness, thread, connection)
}

fn notify(
    harness: &DesktopHarness,
    cx: &mut TestAppContext,
    thread: &Entity<Thread>,
    connection: &ConnectionTo<Client>,
    notification: impl JsonRpcNotification,
) {
    let revision = thread.read_with(cx, |thread, _| {
        thread.core().unwrap().conversation().items().last().map(ConversationItem::revision)
    });
    connection.send_notification(notification).unwrap();
    harness.run_until(cx, |cx| {
        thread.read(cx).core().unwrap().conversation().items().last().map(ConversationItem::revision) != revision
    });
}
