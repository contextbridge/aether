use crate::support::{asked, asking_name};
use acp_utils::testing::{FakeAgent, message_chunk, plan, thought_chunk, tool_call};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::schema::v2 as acp;
use gpui::TestAppContext;

#[gpui::test]
fn a_running_turn_says_what_the_agent_is_doing_and_how_long_it_took_once_it_ends(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    assert!(harness.bounds(cx, "activity").is_none(), "an idle thread shows no activity");
    assert!(harness.bounds(cx, "last-turn").is_none(), "no turn has run yet");

    let turn = harness.start_turn(cx, &thread, &mut requests, "work");
    assert!(harness.bounds(cx, "activity").is_some(), "the turn shows activity before the first token");
    assert!(harness.bounds(cx, "last-turn").is_none(), "the running turn shows its clock instead");

    turn.update(message_chunk("message-0", "Let me look"));
    harness.run_until_painted(cx, "activity-writing");

    turn.update(thought_chunk("thought-0", "pondering"));
    harness.run_until_painted(cx, "activity-thinking");

    turn.update(tool_call("call-1", "Read file", acp::ToolCallStatus::InProgress));
    harness.run_until_painted(cx, "activity-working");
    assert!(harness.shows(cx, "status-label", "Working"), "the running tool is left to the transcript");

    turn.end(acp::StopReason::EndTurn);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);

    assert!(harness.bounds(cx, "activity").is_none(), "the activity ends with the turn");
    assert!(harness.bounds(cx, "last-turn").is_some());
    assert!(harness.bounds(cx, "elapsed").is_none(), "the clock stops with the turn");
}

#[gpui::test]
fn the_status_names_the_plan_step_in_progress_without_moving_or_resizing_anything(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).window_size(900.0, 600.0).start(cx, "/alpha");
    let transcript = harness.bounds(cx, "transcript");
    let turn = harness.start_turn(cx, &thread, &mut requests, "work");
    harness.run_until_painted(cx, "activity");
    let (status, composer) = (harness.bounds(cx, "status"), harness.bounds(cx, "composer"));

    let long = ["Thread trailing-comma tolerance through every list-like production in the grammar"; 4].join(", ");
    let in_progress = entry(&long, acp::PlanEntryStatus::InProgress);
    turn.update(plan("plan", vec![entry("write tests", acp::PlanEntryStatus::Completed), in_progress]));
    harness.run_until_painted(cx, "plan-toggle");

    assert!(harness.shows(cx, "status-label", &long));
    assert_eq!(harness.bounds(cx, "status"), status, "a long label truncates instead of growing");
    assert_eq!(harness.bounds(cx, "composer"), composer, "the composer stays put");
    let elapsed = harness.bounds(cx, "elapsed").expect("the turn's clock");
    let toggle = harness.bounds(cx, "plan-toggle").expect("the plan's progress");
    assert!(toggle.right() <= elapsed.left(), "the status never runs into its clock");

    harness.click(cx, "plan-toggle").unwrap();
    assert!(harness.bounds(cx, "plan-panel").is_some());
    assert_eq!(harness.bounds(cx, "transcript"), transcript, "the open plan floats over the transcript");

    turn.update(plan("plan", vec![entry("fix bug", acp::PlanEntryStatus::Completed)]));
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    assert!(harness.bounds(cx, "plan-toggle").is_none());
    assert_eq!(harness.bounds(cx, "transcript"), transcript, "the status leaves without resizing it");
}

#[gpui::test]
fn the_status_and_the_chat_tab_say_when_the_agent_waits_on_the_user(cx: &mut TestAppContext) {
    let (harness, _) = asked(cx, asking_name());

    harness.run_until_painted(cx, "activity-waiting");
    assert!(harness.shows(cx, "status-label", "Waiting for you"));

    harness.simulate_keystrokes(cx, "secondary-2");
    assert!(harness.bounds(cx, "chat-waiting").is_some(), "the chat tab marks the question");
    assert!(harness.bounds(cx, "elicitation").is_none(), "the question waits in the chat");
}

#[gpui::test]
fn the_chat_tab_marks_an_agent_at_work_while_another_tab_shows(cx: &mut TestAppContext) {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (harness, thread) = DesktopHarness::alpha(cx, agent);
    let turn = harness.start_turn(cx, &thread, &mut requests, "work");
    assert!(harness.bounds(cx, "chat-working").is_none(), "the chat itself shows the work");

    harness.simulate_keystrokes(cx, "secondary-2");
    assert!(harness.bounds(cx, "chat-working").is_some(), "the chat tab marks the work");

    turn.end(acp::StopReason::EndTurn);
    harness.run_until(cx, |cx| thread.read(cx).status() == ThreadStatus::Idle);
    assert!(harness.bounds(cx, "chat-working").is_none(), "the mark ends with the turn");
}

fn entry(content: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
    acp::PlanEntry::new(content, acp::PlanEntryPriority::Medium, status)
}
