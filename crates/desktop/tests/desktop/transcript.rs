use crate::support::replying;
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use agent_client_protocol::schema::v2 as acp;
use gpui::{ScrollDelta, TestAppContext, point, px};

#[gpui::test]
fn a_code_block_in_a_reply_copies_its_code(cx: &mut TestAppContext) {
    let reply = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nRun it.\n";
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", reply)).build(cx);
    let thread = harness.start_thread(cx, "/alpha");

    harness.send(cx, &thread, "how?");
    harness.click(cx, "copy-code-1-0").unwrap();

    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("fn main() {\n    println!(\"hi\");\n}"));
}

#[gpui::test]
fn each_code_block_has_its_own_copy_button(cx: &mut TestAppContext) {
    let reply = "```\nfirst\n```\n\nand\n\n```sh\nsecond\n```\n";
    // Copy buttons are tagged by their block's byte offset in the reply.
    assert_eq!(reply.find("```sh"), Some(20));
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", reply)).build(cx);
    let thread = harness.start_thread(cx, "/alpha");

    harness.send(cx, &thread, "two blocks");
    harness.click(cx, "copy-code-1-20").unwrap();

    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("second"));
}

#[gpui::test]
fn a_reply_streamed_in_chunks_is_as_tall_as_the_same_reply_sent_whole(cx: &mut TestAppContext) {
    let chunks = ["# Plan\n\nFirst ", "step.\n\n- one\n", "- two\n\n```sh\nls\n```\n"];
    let streamed = replying_with(cx, chunks.iter().map(|chunk| message(chunk)).collect());
    let whole = replying_with(cx, vec![message(&chunks.concat())]);
    let short = replying_with(cx, vec![message("Done.")]);

    let height =
        |harness: &DesktopHarness, cx: &mut TestAppContext| harness.bounds(cx, "transcript-row-1").unwrap().size.height;
    assert_eq!(height(&streamed, cx), height(&whole, cx));
    assert!(height(&streamed, cx) > height(&short, cx), "the streamed blocks are all laid out");
}

#[gpui::test]
fn a_reply_can_be_selected_and_copied(cx: &mut TestAppContext) {
    let harness = replying_with(cx, vec![message("Some **bold** advice.")]);

    let row = harness.bounds(cx, "transcript-row-1").unwrap();
    harness.drag(cx, "transcript-row-1", point(row.left() + px(4.0), row.center().y)).unwrap();
    harness.simulate_keystrokes(cx, "secondary-a secondary-c");

    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()).as_deref(), Some("Some bold advice."));
}

#[gpui::test]
fn a_tool_card_unfolds_its_input_and_output(cx: &mut TestAppContext) {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from("file contents")]);
    let harness = replying_with(cx, vec![acp::SessionUpdate::ToolCallUpdate(tool)]);
    assert!(harness.bounds(cx, "tool-card-1").is_some(), "the tool call follows the prompt");
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "a tool card starts folded");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_some(), "clicking unfolds it");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "clicking again folds it");
}

#[gpui::test]
fn a_focused_tool_row_can_be_toggled_with_the_keyboard(cx: &mut TestAppContext) {
    let tool = acp::ToolCallUpdate::new("call-1").title("Read file").status(acp::ToolCallStatus::Completed);
    let harness = replying_with(cx, vec![acp::SessionUpdate::ToolCallUpdate(tool)]);
    harness.click(cx, "tool-card-1").unwrap();
    harness.hover(cx, "sidebar").unwrap();

    harness.press_key(cx, "space");
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "Space folds the focused row");

    harness.press_key(cx, "enter");
    assert!(harness.bounds(cx, "tool-details-1").is_some(), "Enter unfolds the focused row");
}

#[gpui::test]
fn a_multi_line_tool_title_stays_within_its_row(cx: &mut TestAppContext) {
    let command = "python3 - <<'EOF'\nprint('one')\nprint('two')\nprint('three')\nEOF\necho ok";
    let tool = acp::ToolCallUpdate::new("call-1").title(command).status(acp::ToolCallStatus::Completed);
    let harness = replying_with(cx, vec![acp::SessionUpdate::ToolCallUpdate(tool)]);

    let row = harness.bounds(cx, "tool-card-1").unwrap();
    let title = harness.bounds(cx, "tool-title-1").unwrap();
    assert!(title.top() >= row.top() && title.bottom() <= row.bottom(), "{title:?} spills out of {row:?}");
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
    let harness =
        replying_with(cx, vec![acp::SessionUpdate::ToolCallUpdate(edit), acp::SessionUpdate::ToolCallUpdate(read)]);

    assert!(harness.bounds(cx, "tool-change-summary-1").is_some(), "the edit summarizes its changes while folded");
    assert!(harness.bounds(cx, "tool-changes-1").is_none(), "patches start folded");
    assert!(harness.bounds(cx, "tool-change-summary-2").is_none(), "a read has no change summary");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-changes-1").is_some(), "expanding reveals the patch");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-changes-1").is_none(), "folding hides the patch again");
}

#[gpui::test]
fn reasoning_unfolds_to_show_its_text(cx: &mut TestAppContext) {
    let harness = replying_with(
        cx,
        vec![
            acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new("pondering".into(), "thought-0".to_owned())),
            acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new("done".into(), "message-0".to_owned())),
        ],
    );
    assert!(harness.bounds(cx, "thinking-text-1").is_none(), "reasoning starts folded");

    harness.click(cx, "thinking-1").unwrap();

    assert!(harness.bounds(cx, "thinking-text-1").is_some());
}

#[gpui::test]
fn the_plan_stays_in_view_while_it_has_work_left_and_hides_once_finished(cx: &mut TestAppContext) {
    let working =
        plan(&[("write tests", acp::PlanEntryStatus::Completed), ("fix bug", acp::PlanEntryStatus::InProgress)]);
    let finished = plan(&[("fix bug", acp::PlanEntryStatus::Completed)]);
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", updating("alpha-session", vec![working]))
        .agent("/beta", updating("beta-session", vec![finished]))
        .build(cx);
    let alpha = harness.start_thread(cx, "/alpha");
    assert!(harness.bounds(cx, "plan-panel").is_none(), "there is no plan yet");

    harness.send(cx, &alpha, "plan it");
    assert!(harness.bounds(cx, "plan-entry-0").is_some());
    assert!(harness.bounds(cx, "plan-entry-1").is_some());

    harness.click(cx, "plan-header").unwrap();
    assert!(harness.bounds(cx, "plan-header").is_some(), "the plan folds down to its header");
    assert!(harness.bounds(cx, "plan-entry-0").is_none());

    let beta = harness.start_thread(cx, "/beta");
    harness.send(cx, &beta, "plan it");
    assert!(harness.bounds(cx, "plan-panel").is_none(), "a finished plan hides once the turn ends");
}

#[gpui::test]
fn a_long_transcript_follows_its_latest_reply_until_scrolled_away(cx: &mut TestAppContext) {
    let reply = (0..200).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n\n");
    let harness = DesktopHarnessBuilder::default()
        .agent("/alpha", replying("alpha-session", &reply))
        .window_size(900.0, 600.0)
        .build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    harness.send(cx, &thread, "talk a lot");
    let track = harness.bounds(cx, "transcript-scrollbar").expect("a scrollbar beside the transcript");
    let thumb = harness.bounds(cx, "transcript-scrollbar-thumb").expect("a thumb on the scrollbar");
    assert_eq!(thumb.bottom(), track.bottom(), "the transcript follows its latest reply");
    assert!(harness.bounds(cx, "jump-to-latest").is_none());

    harness.scroll(cx, "transcript", ScrollDelta::Lines(point(0.0, 10.0))).unwrap();
    assert!(harness.bounds(cx, "jump-to-latest").is_some(), "scrolling up stops following");
    harness.click(cx, "jump-to-latest").unwrap();
    assert!(harness.bounds(cx, "jump-to-latest").is_none(), "jumping back follows again");

    harness.drag(cx, "transcript-scrollbar-thumb", point(track.center().x, track.top())).unwrap();
    assert!(harness.bounds(cx, "jump-to-latest").is_some(), "dragging up stops following");
    harness.drag(cx, "transcript-scrollbar-thumb", point(track.center().x, track.bottom())).unwrap();
    assert!(harness.bounds(cx, "jump-to-latest").is_none(), "dragging to the bottom follows again");
}

#[gpui::test]
fn a_short_transcript_has_no_scrollbar(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", replying("alpha-session", "Hello")).build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    harness.send(cx, &thread, "hi");

    assert!(harness.bounds(cx, "transcript-scrollbar").is_none());
}

/// An agent that opens `session_id` and answers every prompt with `updates`.
fn updating(session_id: &str, updates: Vec<acp::SessionUpdate>) -> FakeAgent {
    FakeAgent::default().new_session_response(acp::NewSessionResponse::new(session_id)).prompt_updates(updates)
}

/// A thread in `/alpha` whose agent answered one prompt with `updates`.
fn replying_with(cx: &mut TestAppContext, updates: Vec<acp::SessionUpdate>) -> DesktopHarness {
    let harness = DesktopHarnessBuilder::default().agent("/alpha", updating("alpha-session", updates)).build(cx);
    let thread = harness.start_thread(cx, "/alpha");
    harness.send(cx, &thread, "go");
    harness
}

fn message(text: &str) -> acp::SessionUpdate {
    acp::SessionUpdate::AgentMessageChunk(acp::ContentChunk::new(text.into(), "message-0".to_owned()))
}

fn plan(entries: &[(&str, acp::PlanEntryStatus)]) -> acp::SessionUpdate {
    let entries = entries
        .iter()
        .map(|(content, status)| acp::PlanEntry::new(*content, acp::PlanEntryPriority::Medium, status.clone()))
        .collect();
    acp::SessionUpdate::PlanUpdate(acp::PlanUpdate::new(acp::PlanUpdateContent::items("plan", entries)))
}
