use crate::support::{replying, start_thread};
use acp_utils::testing::FakeAgent;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder, FakeConnector};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2 as acp;
use gpui::{Entity, TestAppContext};

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
fn a_tool_card_shows_the_file_changes_its_tool_made(cx: &mut TestAppContext) {
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

    assert!(harness.bounds(cx, "tool-changes-1").is_some(), "the edit shows its patch while folded");
    assert!(harness.bounds(cx, "tool-changes-2").is_none(), "a tool that changed nothing shows no changes");
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
