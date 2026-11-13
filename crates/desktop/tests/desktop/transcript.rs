use crate::support::Edges;
use acp_utils::testing::{
    FakeAgent, FakeAgentRequests, announced, message_chunk, plan, running_state, thought_chunk, tool_call,
};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::{Thread, ThreadStatus};
use agent_client_protocol::v2::schema as acp;
use rouge::core::Point;
use std::fmt::Write;

#[test]
fn each_code_block_in_a_reply_copies_its_own_code() {
    let reply = "```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nRun it.\n\n```sh\nsecond\n```\n";
    let mut harness = answered(vec![message(reply)]);

    harness.click_nth("markdown-copy-code", 0).unwrap();
    assert_eq!(harness.platform().copied_text().as_deref(), Some("fn main() {\n    println!(\"hi\");\n}\n"));

    harness.click_nth("markdown-copy-code", 1).unwrap();
    assert_eq!(harness.platform().copied_text().as_deref(), Some("second\n"));
}

#[test]
fn links_in_a_reply_open_when_clicked() {
    let reply = "Read [the guide](https://example.com/guide) or https://docs.rs/rouge.";
    let mut harness = answered(vec![message(reply)]);

    harness.click_text("the guide").unwrap();
    harness.click_text("docs.rs").unwrap();
    harness.click_text("Read").unwrap();

    assert_eq!(harness.platform().opened(), ["https://example.com/guide", "https://docs.rs/rouge"]);
}

#[test]
fn a_reply_streamed_in_chunks_is_as_tall_as_the_same_reply_sent_whole() {
    let chunks = ["# Plan\n\nFirst ", "step.\n\n- one\n", "- two\n\n```sh\nls\n```\n"];
    let height = |updates| answered(updates).bounds("transcript-row-1").unwrap().size.height;

    let streamed = height(chunks.iter().map(|chunk| message(chunk)).collect());
    assert_eq!(streamed, height(vec![message(&chunks.concat())]));
    assert!(streamed > height(vec![message("Done.")]), "the streamed blocks are all laid out");
}

#[test]
fn a_prompt_or_reply_copies_its_text() {
    let mut harness = answered(vec![message("Some **bold** advice.")]);

    harness.click("copy-message-1").unwrap();
    assert_eq!(harness.platform().copied_text().as_deref(), Some("Some **bold** advice."));

    harness.click("copy-message-0").unwrap();
    assert_eq!(harness.platform().copied_text().as_deref(), Some("go"));
}

#[test]
fn a_run_of_finished_tool_calls_folds_into_one_group() {
    let mut harness = answered(finished_tools(&["Read a", "Read b", "Edit c"]));
    assert!(harness.bounds("tool-group-1").is_some());
    assert!(harness.bounds("tool-card-2").is_none(), "the group starts folded");

    harness.click("tool-group-1").unwrap();
    for card in ["tool-card-1", "tool-card-2", "tool-card-3"] {
        assert!(harness.bounds(card).is_some(), "unfolding the group shows {card}");
    }
    harness.click("tool-card-2").unwrap();
    assert!(harness.bounds("tool-details-2").is_some(), "a call in the group still unfolds");

    harness.click("tool-group-1").unwrap();
    assert!(harness.bounds("tool-card-2").is_none(), "folding the group hides its calls again");
}

#[test]
fn a_couple_of_tool_calls_without_a_reply_stay_ungrouped_and_in_view() {
    let mut harness = answered(finished_tools(&["Read a", "Read b"]));

    assert!(harness.bounds("tool-group-1").is_none());
    assert!(harness.bounds("turn-summary-1").is_none());
    assert!(harness.bounds("tool-card-1").is_some());
    assert!(harness.bounds("tool-card-2").is_some());
}

#[test]
fn a_finished_turn_keeps_its_work_in_view_until_the_next_prompt_folds_it_above_its_reply() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    play_turn(&mut harness, thread, &mut requests, finished_tools(&["Read a", "Read b"]), "Done.");
    assert!(harness.bounds("tool-card-1").is_some(), "the work the user just watched stays in view");
    assert!(harness.bounds("turn-summary-1").is_none());

    let _next = harness.start_turn(thread, &mut requests, "again");
    assert!(harness.bounds("turn-summary-1").is_some());
    assert!(harness.bounds("tool-card-1").is_none(), "the turn's work starts folded");
    assert!(harness.bounds("reply-3").is_some(), "the reply stays in view");

    harness.click("turn-summary-1").unwrap();
    for card in ["tool-card-1", "tool-card-2"] {
        assert!(harness.bounds(card).is_some(), "unfolding the turn shows {card}");
    }

    harness.click("turn-summary-1").unwrap();
    assert!(harness.bounds("tool-card-1").is_none(), "folding the turn hides its work again");
}

#[test]
fn a_failed_prompt_shows_as_an_error() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").hold::<acp::PromptRequest>().capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);

    thread.send("hi".into());
    let (_, responder) = harness.next(requests.held::<acp::PromptRequest>());
    responder.respond_with_error(agent_client_protocol::Error::internal_error()).unwrap();

    harness.run_until_painted("transcript-error-0");
}

#[test]
fn a_tool_card_unfolds_its_input_and_output() {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from("file contents")]);
    let mut harness = answered(vec![acp::SessionUpdate::ToolCallUpdate(tool)]);
    assert!(harness.bounds("tool-card-1").is_some(), "the tool call follows the prompt");
    assert!(harness.bounds("tool-details-1").is_none(), "a tool card starts folded");

    harness.click("tool-card-1").unwrap();
    assert!(harness.bounds("tool-details-1").is_some(), "clicking unfolds it");

    harness.click("tool-card-1").unwrap();
    assert!(harness.bounds("tool-details-1").is_none(), "clicking again folds it");
}

#[test]
fn a_focused_tool_row_can_be_toggled_with_the_keyboard() {
    let mut harness = answered(vec![tool_call("call-1", "Read file", acp::ToolCallStatus::Completed)]);
    harness.click("tool-card-1").unwrap();
    harness.hover("sidebar").unwrap();

    harness.press("space");
    assert!(harness.bounds("tool-details-1").is_none(), "Space folds the focused row");

    harness.press("enter");
    assert!(harness.bounds("tool-details-1").is_some(), "Enter unfolds the focused row");
}

#[test]
fn a_multi_line_tool_title_stays_within_its_row() {
    let command = "python3 - <<'EOF'\nprint('one')\nprint('two')\nprint('three')\nEOF\necho ok";
    let mut harness = answered(vec![tool_call("call-1", command, acp::ToolCallStatus::Completed)]);

    let row = harness.bounds("tool-card-1").unwrap();
    let title = harness.bounds("tool-title-1").unwrap();
    assert!(title.top() >= row.top() && title.bottom() <= row.bottom(), "{title:?} spills out of {row:?}");
}

#[test]
fn a_tool_row_summarizes_changes_and_only_unfolds_the_patch_on_demand() {
    let patch = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
    let diff = acp::Diff::patch(patch, vec![acp::DiffChange::modify("/alpha/src/lib.rs")]);
    let edit = acp::ToolCallUpdate::new("call-1")
        .title("Edit file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from(diff)]);
    let read = tool_call("call-2", "Read file", acp::ToolCallStatus::Completed);
    let mut harness = answered(vec![acp::SessionUpdate::ToolCallUpdate(edit), read]);

    assert!(harness.bounds("tool-change-summary-1").is_some(), "the edit summarizes its changes while folded");
    assert!(harness.bounds("tool-changes-1").is_none(), "patches start folded");
    assert!(harness.bounds("tool-change-summary-2").is_none(), "a read has no change summary");

    harness.click("tool-card-1").unwrap();
    assert!(harness.bounds("tool-changes-1").is_some(), "expanding reveals the patch");

    harness.click("tool-card-1").unwrap();
    assert!(harness.bounds("tool-changes-1").is_none(), "folding hides the patch again");
}

#[test]
fn a_long_patch_previews_its_start_and_opens_in_full_in_a_diff_viewer() {
    let mut harness = answered(vec![write_tool("call-1", Some(added_file(1000)), "")]);
    harness.click("tool-card-1").unwrap();
    assert!(harness.bounds("tool-diff-1-0").is_some(), "the patch previews in the card");

    harness.scroll_into_view("transcript-view", "tool-diff-1-0-more").unwrap();
    harness.click("tool-diff-1-0-more").unwrap();
    assert!(harness.bounds("full-diff-panel").is_some(), "the whole patch opens in a diff viewer");

    harness.click("close-full-diff").unwrap();
    assert!(harness.bounds("full-diff-panel").is_none(), "closing returns to the transcript");

    harness.click("tool-diff-1-0-more").unwrap();
    harness.press("escape");
    assert!(harness.bounds("full-diff-panel").is_none(), "Escape closes the diff viewer");
}

#[test]
fn a_short_patch_previews_in_full() {
    let mut harness = answered(vec![write_tool("call-1", Some(added_file(5)), "")]);
    harness.click("tool-card-1").unwrap();

    assert!(harness.bounds("tool-diff-1-0").is_some());
    assert!(harness.bounds("tool-diff-1-0-more").is_none());
}

#[test]
fn a_long_tool_output_is_cut_short_until_shown_in_full() {
    let tool = acp::ToolCallUpdate::new("call-1")
        .title("Read file")
        .status(acp::ToolCallStatus::Completed)
        .content(vec![acp::ToolCallContent::from(numbered_lines(1000))]);
    let mut harness = answered(vec![acp::SessionUpdate::ToolCallUpdate(tool)]);
    harness.click("tool-card-1").unwrap();
    let cut = harness.bounds("tool-details-1").unwrap().size.height;

    harness.scroll_into_view("transcript-view", "tool-output-more-1").unwrap();
    harness.click("tool-output-more-1").unwrap();

    assert!(harness.bounds("tool-output-more-1").is_none(), "the whole output shows");
    assert!(harness.bounds("tool-details-1").unwrap().size.height > cut);
}

#[test]
fn a_tool_with_a_patch_does_not_repeat_the_patched_text_as_its_input() {
    let input = numbered_lines(1000);
    let mut patched = answered(vec![write_tool("call-1", Some(added_file(5)), &input)]);
    patched.click("tool-card-1").unwrap();
    assert!(patched.bounds("tool-details-1").is_some());
    assert!(patched.bounds("tool-input-more-1").is_none(), "the patch already shows the change");
    drop(patched);

    let mut unpatched = answered(vec![write_tool("call-1", None, &input)]);
    unpatched.click("tool-card-1").unwrap();
    assert!(unpatched.bounds("tool-input-more-1").is_some(), "without a patch the input shows, cut short");
}

#[test]
fn reasoning_unfolds_to_show_its_text() {
    let mut harness = answered(vec![thought_chunk("thought-0", "pondering"), message_chunk("message-0", "done")]);
    assert!(harness.bounds("thinking-text-1").is_none(), "reasoning starts folded");

    harness.click("thinking-1").unwrap();

    assert!(harness.bounds("thinking-text-1").is_some());
}

#[test]
fn the_plan_shows_its_progress_while_it_has_work_left_and_hides_once_finished() {
    let working = plan(
        "plan",
        vec![entry("write tests", acp::PlanEntryStatus::Completed), entry("fix bug", acp::PlanEntryStatus::InProgress)],
    );
    let finished = plan("plan", vec![entry("fix bug", acp::PlanEntryStatus::Completed)]);
    let (mut harness, alpha) = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::new("alpha-session").prompt_updates(vec![working]))
        .agent("/beta", FakeAgent::new("beta-session").prompt_updates(vec![finished]))
        .start("/alpha");
    assert!(harness.bounds("plan-toggle").is_none(), "there is no plan yet");

    harness.send(alpha, "plan it");
    assert!(harness.bounds("plan-toggle").is_some());
    assert!(harness.bounds("plan-entry-0").is_none(), "the plan starts folded to its progress");

    harness.click("plan-toggle").unwrap();
    assert!(harness.bounds("plan-entry-0").is_some());
    assert!(harness.bounds("plan-entry-1").is_some());

    harness.click("plan-toggle").unwrap();
    assert!(harness.bounds("plan-entry-0").is_none(), "the plan folds back to its progress");

    let beta = harness.start_thread("/beta");
    harness.send(beta, "plan it");
    assert!(harness.bounds("plan-toggle").is_none(), "a finished plan hides once the turn ends");
}

#[test]
fn a_long_transcript_follows_its_latest_reply_until_scrolled_away() {
    let reply = (0..200).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n\n");
    let (mut harness, thread) = DesktopHarnessBuilder::default()
        .agent("/alpha", FakeAgent::new("alpha-session").prompt_reply(&reply))
        .window_size(900.0, 600.0)
        .start("/alpha");
    harness.send(thread, "talk a lot");
    let track = harness.bounds("transcript-scrollbar").expect("a scrollbar beside the transcript");
    let thumb = harness.bounds("scroll-thumb-vertical").expect("a thumb on the scrollbar");
    assert_eq!(thumb.bottom(), track.bottom(), "the transcript follows its latest reply");
    assert!(harness.bounds("jump-to-latest").is_none());

    harness.scroll("transcript", Point::new(0.0, 200.0)).unwrap();
    assert!(harness.bounds("jump-to-latest").is_some(), "scrolling up stops following");
    harness.click("jump-to-latest").unwrap();
    assert!(harness.bounds("jump-to-latest").is_none(), "jumping back follows again");

    harness.drag("scroll-thumb-vertical", Point::new(track.center().x, track.top())).unwrap();
    assert!(harness.bounds("jump-to-latest").is_some(), "dragging up stops following");
    harness.drag("scroll-thumb-vertical", Point::new(track.center().x, track.bottom())).unwrap();
    assert!(harness.bounds("jump-to-latest").is_none(), "dragging to the bottom follows again");
}

#[test]
fn a_short_transcript_has_no_scrollbar() {
    let mut harness = answered(vec![message("Hello")]);

    assert!(harness.bounds("transcript-scrollbar").is_none());
}

#[test]
fn a_tool_whose_sub_agent_still_runs_is_never_folded_away() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let turn = harness.start_turn(thread, &mut requests, "go");

    for update in finished_tools(&["Read a", "Read b", "spawn_subagent"]) {
        turn.update(update);
    }
    let child = acp::SubagentUpdate::new("child-a").state(running_state());
    harness.next(&mut requests.connection).send_notification(announced("alpha-session", "call-2", child)).unwrap();
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(|_| thread.status() == ThreadStatus::Background);

    assert!(harness.bounds("tool-group-1").is_none(), "a run with live work in it is not folded");
    assert!(harness.bounds("tool-card-3").is_some(), "the running sub-agent's tool shows");
}

fn answered(updates: Vec<acp::SessionUpdate>) -> DesktopHarness {
    let (mut harness, thread) = DesktopHarness::alpha(FakeAgent::new("alpha-session").prompt_updates(updates));
    harness.send(thread, "go");
    harness
}

fn play_turn(
    harness: &mut DesktopHarness,
    thread: Thread,
    requests: &mut FakeAgentRequests,
    work: Vec<acp::SessionUpdate>,
    reply: &str,
) {
    let turn = harness.start_turn(thread, requests, "go");
    for update in work {
        turn.update(update);
    }
    turn.reply(reply);
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);
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

fn write_tool(id: &str, patch: Option<String>, input: &str) -> acp::SessionUpdate {
    let content: Vec<_> = patch
        .map(|patch| acp::Diff::patch(patch, vec![acp::DiffChange::add("/alpha/page.html")]))
        .map(acp::ToolCallContent::from)
        .into_iter()
        .collect();
    let tool = acp::ToolCallUpdate::new(id.to_owned())
        .title("Write page.html")
        .status(acp::ToolCallStatus::Completed)
        .raw_input(serde_json::Value::String(input.to_owned()))
        .content(content);
    acp::SessionUpdate::ToolCallUpdate(tool)
}

fn added_file(lines: usize) -> String {
    let body = (0..lines).fold(String::new(), |mut body, line| {
        let _ = writeln!(body, "+<p>line {line}</p>");
        body
    });
    format!(
        "diff --git a/page.html b/page.html\nnew file mode 100644\n--- /dev/null\n+++ b/page.html\n@@ -0,0 +1,{lines} @@\n{body}"
    )
}

fn numbered_lines(lines: usize) -> String {
    (0..lines).map(|line| format!("line {line}")).collect::<Vec<_>>().join("\n")
}
