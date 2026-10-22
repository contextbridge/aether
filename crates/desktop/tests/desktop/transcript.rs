use crate::support::{replying, start_thread};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2 as acp;
use gpui::{Entity, TestAppContext, px};

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
    let additions = "+new line\n".repeat(24);
    let patch = format!("+++ b/src/lib.rs\n@@ -0,0 +1,24 @@\n{additions}");
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
