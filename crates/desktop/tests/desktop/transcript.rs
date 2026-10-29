use acp_utils::testing::{
    FakeAgent, FakeAgentRequests, announced, message_chunk, plan, running_state, thought_chunk, tool_call,
};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::schema::v2 as acp;
use gpui::{Entity, ScrollDelta, TestAppContext, point, px};

#[gpui::test]
fn each_code_block_in_a_reply_copies_its_own_code(cx: &mut TestAppContext) {
    let reply = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nRun it.\n\n```sh\nsecond\n```\n";
    assert_eq!(reply.find("```sh"), Some(56));
    let harness = answered(cx, vec![message(reply)]);

    harness.click(cx, "copy-code-1-0").unwrap();
    assert_eq!(copied(cx).as_deref(), Some("fn main() {\n    println!(\"hi\");\n}"));

    harness.click(cx, "copy-code-1-56").unwrap();
    assert_eq!(copied(cx).as_deref(), Some("second"));
}

#[gpui::test]
fn a_reply_streamed_in_chunks_is_as_tall_as_the_same_reply_sent_whole(cx: &mut TestAppContext) {
    let chunks = ["# Plan\n\nFirst ", "step.\n\n- one\n", "- two\n\n```sh\nls\n```\n"];
    let streamed = answered(cx, chunks.iter().map(|chunk| message(chunk)).collect());
    let whole = answered(cx, vec![message(&chunks.concat())]);
    let short = answered(cx, vec![message("Done.")]);

    let height =
        |harness: &DesktopHarness, cx: &mut TestAppContext| harness.bounds(cx, "transcript-row-1").unwrap().size.height;
    assert_eq!(height(&streamed, cx), height(&whole, cx));
    assert!(height(&streamed, cx) > height(&short, cx), "the streamed blocks are all laid out");
}

#[gpui::test]
fn a_reply_can_be_selected_and_copied_and_a_prompt_or_reply_copies_its_text(cx: &mut TestAppContext) {
    let harness = answered(cx, vec![message("Some **bold** advice.")]);

    let reply = harness.bounds(cx, "reply-1").unwrap();
    harness.drag(cx, "reply-1", point(reply.left() + px(4.0), reply.center().y)).unwrap();
    harness.simulate_keystrokes(cx, "secondary-a secondary-c");
    assert_eq!(copied(cx).as_deref(), Some("Some bold advice."));

    harness.click(cx, "copy-message-1").unwrap();
    assert_eq!(copied(cx).as_deref(), Some("Some **bold** advice."));

    harness.click(cx, "copy-message-0").unwrap();
    assert_eq!(copied(cx).as_deref(), Some("go"));
}

#[gpui::test]
fn a_run_of_finished_tool_calls_folds_into_one_group(cx: &mut TestAppContext) {
    let harness = answered(cx, finished_tools(&["Read a", "Read b", "Edit c"]));
    assert!(harness.bounds(cx, "tool-group-1").is_some());
    assert!(harness.bounds(cx, "tool-card-2").is_none(), "the group starts folded");

    harness.click(cx, "tool-group-1").unwrap();
    for card in ["tool-card-1", "tool-card-2", "tool-card-3"] {
        assert!(harness.bounds(cx, card).is_some(), "unfolding the group shows {card}");
    }
    harness.click(cx, "tool-card-2").unwrap();
    assert!(harness.bounds(cx, "tool-details-2").is_some(), "a call in the group still unfolds");

    harness.click(cx, "tool-group-1").unwrap();
    assert!(harness.bounds(cx, "tool-card-2").is_none(), "folding the group hides its calls again");
}

#[gpui::test]
fn a_couple_of_tool_calls_without_a_reply_stay_ungrouped_and_in_view(cx: &mut TestAppContext) {
    let harness = answered(cx, finished_tools(&["Read a", "Read b"]));

    assert!(harness.bounds(cx, "tool-group-1").is_none());
    assert!(harness.bounds(cx, "turn-summary-1").is_none());
    assert!(harness.bounds(cx, "tool-card-1").is_some());
    assert!(harness.bounds(cx, "tool-card-2").is_some());
}

#[gpui::test]
fn a_finished_turn_keeps_its_work_in_view_until_the_next_prompt_folds_it_above_its_reply(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    play_turn(&harness, cx, &thread, &mut requests, finished_tools(&["Read a", "Read b"]), "Done.");
    assert!(harness.bounds(cx, "tool-card-1").is_some(), "the work the user just watched stays in view");
    assert!(harness.bounds(cx, "turn-summary-1").is_none());

    let _next = harness.start_turn(cx, &thread, &mut requests, "again");
    assert!(harness.bounds(cx, "turn-summary-1").is_some());
    assert!(harness.bounds(cx, "tool-card-1").is_none(), "the turn's work starts folded");
    assert!(harness.bounds(cx, "reply-3").is_some(), "the reply stays in view");

    harness.click(cx, "turn-summary-1").unwrap();
    for card in ["tool-card-1", "tool-card-2"] {
        assert!(harness.bounds(cx, card).is_some(), "unfolding the turn shows {card}");
    }

    harness.click(cx, "turn-summary-1").unwrap();
    assert!(harness.bounds(cx, "tool-card-1").is_none(), "folding the turn hides its work again");
}

#[gpui::test]
fn a_failed_prompt_shows_as_an_error(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").hold::<acp::PromptRequest>().capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);

    thread.update(cx, |thread, cx| thread.send("hi".into(), cx));
    let (_, responder) = harness.next(cx, requests.held::<acp::PromptRequest>());
    responder.respond_with_error(agent_client_protocol::Error::internal_error()).unwrap();

    harness.run_until_painted(cx, "transcript-error-0");
}

#[gpui::test]
fn a_tool_card_unfolds_its_input_and_output(cx: &mut TestAppContext) {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from("file contents")]);
    let harness = answered(cx, vec![acp::SessionUpdate::ToolCallUpdate(tool)]);
    assert!(harness.bounds(cx, "tool-card-1").is_some(), "the tool call follows the prompt");
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "a tool card starts folded");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_some(), "clicking unfolds it");

    harness.click(cx, "tool-card-1").unwrap();
    assert!(harness.bounds(cx, "tool-details-1").is_none(), "clicking again folds it");
}

#[gpui::test]
fn a_focused_tool_row_can_be_toggled_with_the_keyboard(cx: &mut TestAppContext) {
    let harness = answered(cx, vec![tool_call("call-1", "Read file", acp::ToolCallStatus::Completed)]);
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
    let harness = answered(cx, vec![tool_call("call-1", command, acp::ToolCallStatus::Completed)]);

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
    let read = tool_call("call-2", "Read file", acp::ToolCallStatus::Completed);
    let harness = answered(cx, vec![acp::SessionUpdate::ToolCallUpdate(edit), read]);

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
    let harness = answered(cx, vec![thought_chunk("thought-0", "pondering"), message_chunk("message-0", "done")]);
    assert!(harness.bounds(cx, "thinking-text-1").is_none(), "reasoning starts folded");

    harness.click(cx, "thinking-1").unwrap();

    assert!(harness.bounds(cx, "thinking-text-1").is_some());
}

#[gpui::test]
fn the_plan_shows_its_progress_while_it_has_work_left_and_hides_once_finished(cx: &mut TestAppContext) {
    let working = plan(
        "plan",
        vec![entry("write tests", acp::PlanEntryStatus::Completed), entry("fix bug", acp::PlanEntryStatus::InProgress)],
    );
    let finished = plan("plan", vec![entry("fix bug", acp::PlanEntryStatus::Completed)]);
    let (harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::new("alpha-session").prompt_updates(vec![working]))
        .agent("/beta", FakeAgent::new("beta-session").prompt_updates(vec![finished]))
        .start(cx, "/alpha");
    assert!(harness.bounds(cx, "plan-toggle").is_none(), "there is no plan yet");

    harness.send(cx, &alpha, "plan it");
    assert!(harness.bounds(cx, "plan-toggle").is_some());
    assert!(harness.bounds(cx, "plan-entry-0").is_none(), "the plan starts folded to its progress");

    harness.click(cx, "plan-toggle").unwrap();
    assert!(harness.bounds(cx, "plan-entry-0").is_some());
    assert!(harness.bounds(cx, "plan-entry-1").is_some());

    harness.click(cx, "plan-toggle").unwrap();
    assert!(harness.bounds(cx, "plan-entry-0").is_none(), "the plan folds back to its progress");

    let beta = harness.start_thread(cx, "/beta");
    harness.send(cx, &beta, "plan it");
    assert!(harness.bounds(cx, "plan-toggle").is_none(), "a finished plan hides once the turn ends");
}

#[gpui::test]
fn a_long_transcript_follows_its_latest_reply_until_scrolled_away(cx: &mut TestAppContext) {
    let reply = (0..200).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n\n");
    let (harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::new("alpha-session").prompt_reply(&reply))
        .window_size(900.0, 600.0)
        .start(cx, "/alpha");
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
    let harness = answered(cx, vec![message("Hello")]);

    assert!(harness.bounds(cx, "transcript-scrollbar").is_none());
}

#[gpui::test]
fn a_tool_whose_sub_agent_still_runs_is_never_folded_away(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    let turn = harness.start_turn(cx, &thread, &mut requests, "go");

    for update in finished_tools(&["Read a", "Read b", "spawn_subagent"]) {
        turn.update(update);
    }
    let child = acp::SubagentUpdate::new("child-a").state(running_state());
    harness.next(cx, &mut requests.connection).send_notification(announced("alpha-session", "call-2", child)).unwrap();
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Background);

    assert!(harness.bounds(cx, "tool-group-1").is_none(), "a run with live work in it is not folded");
    assert!(harness.bounds(cx, "tool-card-3").is_some(), "the running sub-agent's tool shows");
}

fn answered(cx: &mut TestAppContext, updates: Vec<acp::SessionUpdate>) -> DesktopHarness {
    let (harness, thread) = DesktopHarness::alpha(cx, FakeAgent::new("alpha-session").prompt_updates(updates));
    harness.send(cx, &thread, "go");
    harness
}

fn play_turn(
    harness: &DesktopHarness,
    cx: &mut TestAppContext,
    thread: &Entity<Thread>,
    requests: &mut FakeAgentRequests,
    work: Vec<acp::SessionUpdate>,
    reply: &str,
) {
    let turn = harness.start_turn(cx, thread, requests, "go");
    for update in work {
        turn.update(update);
    }
    turn.reply(reply);
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
}

fn copied(cx: &TestAppContext) -> Option<String> {
    cx.read_from_clipboard().and_then(|item| item.text())
}

fn finished_tools(titles: &[&str]) -> Vec<acp::SessionUpdate> {
    titles
        .iter()
        .enumerate()
        .map(|(index, title)| tool_call(&format!("call-{index}"), title, acp::ToolCallStatus::Completed))
        .collect()
}

fn entry(content: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
    acp::PlanEntry::new(content, acp::PlanEntryPriority::Medium, status)
}

fn message(text: &str) -> acp::SessionUpdate {
    message_chunk("message-0", text)
}
