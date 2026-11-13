use crate::support::Edges;
use crate::support::{asked, asking_name};
use acp_utils::testing::{FakeAgent, message_chunk, plan, thought_chunk, tool_call};
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::threads::ThreadStatus;
use agent_client_protocol::v2::schema as acp;

#[test]
fn a_running_turn_says_what_the_agent_is_doing_and_how_long_it_took_once_it_ends() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    assert!(harness.bounds("activity").is_none(), "an idle thread shows no activity");
    assert!(harness.bounds("last-turn").is_none(), "no turn has run yet");

    let turn = harness.start_turn(thread, &mut requests, "work");
    assert!(harness.bounds("activity").is_some(), "the turn shows activity before the first token");
    assert!(harness.bounds("last-turn").is_none(), "the running turn shows its clock instead");

    turn.update(message_chunk("message-0", "Let me look"));
    harness.run_until_painted("activity-writing");

    turn.update(thought_chunk("thought-0", "pondering"));
    harness.run_until_painted("activity-thinking");

    turn.update(tool_call("call-1", "Read file", acp::ToolCallStatus::InProgress));
    harness.run_until_painted("activity-working");
    assert!(harness.shows("status-label", "Working"), "the running tool is left to the transcript");

    turn.end(acp::StopReason::EndTurn);
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);

    assert!(harness.bounds("activity").is_none(), "the activity ends with the turn");
    assert!(harness.bounds("last-turn").is_some());
    assert!(harness.bounds("elapsed").is_none(), "the clock stops with the turn");
}

#[test]
fn the_status_names_the_plan_step_in_progress_without_moving_or_resizing_anything() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) =
        DesktopHarnessBuilder::default().agent("/alpha", agent).window_size(900.0, 600.0).start("/alpha");
    let transcript = harness.bounds("transcript");
    let turn = harness.start_turn(thread, &mut requests, "work");
    harness.run_until_painted("activity");
    let (status, composer) = (harness.bounds("status"), harness.bounds("composer"));

    let long = ["Thread trailing-comma tolerance through every list-like production in the grammar"; 4].join(", ");
    let in_progress = entry(&long, acp::PlanEntryStatus::InProgress);
    turn.update(plan("plan", vec![entry("write tests", acp::PlanEntryStatus::Completed), in_progress]));
    harness.run_until_painted("plan-toggle");

    assert!(harness.shows("status-label", &long));
    assert_eq!(harness.bounds("status"), status, "a long label truncates instead of growing");
    assert_eq!(harness.bounds("composer"), composer, "the composer stays put");
    let elapsed = harness.bounds("elapsed").expect("the turn's clock");
    let toggle = harness.bounds("plan-toggle").expect("the plan's progress");
    assert!(toggle.right() <= elapsed.left(), "the status never runs into its clock");

    harness.click("plan-toggle").unwrap();
    assert!(harness.bounds("plan-panel").is_some());
    assert_eq!(harness.bounds("transcript"), transcript, "the open plan floats over the transcript");

    turn.update(plan("plan", vec![entry("fix bug", acp::PlanEntryStatus::Completed)]));
    turn.end(acp::StopReason::EndTurn);
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);
    assert!(harness.bounds("plan-toggle").is_none());
    assert_eq!(harness.bounds("transcript"), transcript, "the status leaves without resizing it");
}

#[test]
fn the_status_and_the_chat_tab_say_when_the_agent_waits_on_the_user() {
    let (mut harness, _) = asked(asking_name());

    harness.run_until_painted("activity-waiting");
    assert!(harness.shows("status-label", "Waiting for you"));

    harness.press("secondary-2");
    assert!(harness.bounds("chat-waiting").is_some(), "the chat tab marks the question");
    assert!(harness.bounds("elicitation").is_none(), "the question waits in the chat");
}

#[test]
fn activity_details_keep_the_state_and_current_tool_when_a_plan_supplies_the_label() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let turn = harness.start_turn(thread, &mut requests, "work");
    turn.update(plan("plan", vec![entry("Validate authentication", acp::PlanEntryStatus::InProgress)]));
    turn.update(tool_call("call-1", "Running tests", acp::ToolCallStatus::InProgress));
    harness.run_until_painted("activity-working");

    harness.click("activity-details-trigger").unwrap();
    assert!(harness.shows("activity-detail-state", "Working"));
    assert!(harness.shows("activity-detail-plan", "Working on: Validate authentication"));
    assert!(harness.shows("activity-detail-tool", "Running tests"));
    assert!(harness.shows("tool-status-call-1", "Running"));

    turn.update(thought_chunk("thought-1", "considering"));
    harness.run_until_painted("activity-thinking");
    assert!(harness.shows("activity-detail-state", "Thinking"), "open details stay current");
    harness.press("escape");
    assert!(harness.bounds("activity-details").is_none());
    harness.press("enter");
    assert!(harness.bounds("activity-details").is_some(), "the focused trigger opens from the keyboard");
}

#[test]
fn activity_details_are_reachable_by_tab_and_reopen_with_enter_and_space() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let _turn = harness.start_turn(thread, &mut requests, "work");
    assert!(harness.bounds("activity-details").is_none());
    for _ in 0..32 {
        harness.press("tab");
        if harness.bounds("activity-details").is_some() {
            break;
        }
    }
    assert!(harness.bounds("activity-details").is_some(), "Tab can reach and show activity details");
    for key in ["enter", "space"] {
        harness.press("escape");
        assert!(harness.bounds("activity-details").is_none());
        harness.press(key);
        assert!(harness.bounds("activity-details").is_some(), "{key} reopens the focused trigger");
    }
    assert_eq!(thread.status(), ThreadStatus::Running, "dismissing details never cancels the turn");
}

#[test]
fn only_a_newly_completed_plan_step_gets_a_milestone_cue() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let turn = harness.start_turn(thread, &mut requests, "work");
    turn.update(plan(
        "plan",
        vec![entry("Earlier work", acp::PlanEntryStatus::Completed), entry("Tests", acp::PlanEntryStatus::InProgress)],
    ));
    harness.run_until_painted("plan-toggle");
    assert!(harness.bounds("activity-milestone").is_none(), "loading a plan is not a milestone");

    turn.update(thought_chunk("thought-1", "considering"));
    harness.run_until_painted("activity-thinking");
    turn.update(message_chunk("message-1", "Writing"));
    harness.run_until_painted("activity-writing");
    assert!(harness.bounds("activity-milestone").is_none(), "ordinary state changes are quiet");

    turn.update(plan(
        "plan",
        vec![
            entry("Earlier work", acp::PlanEntryStatus::Completed),
            entry("Tests", acp::PlanEntryStatus::Completed),
            entry("Review", acp::PlanEntryStatus::InProgress),
        ],
    ));
    harness.run_until(|harness| harness.shows("status-label", "Review"));
    let first = harness.bounds("activity-milestone").expect("completing a step gets a cue");
    harness.ui().run(0.5);
    let settled = harness.bounds("activity-milestone").unwrap();
    assert!(settled.size.width > first.size.width, "the finite outline expands");
    harness.ui().run(1.0);
    assert_eq!(harness.bounds("activity-milestone"), Some(settled), "the cue does not loop");

    turn.update(plan(
        "plan",
        vec![
            entry("Earlier work", acp::PlanEntryStatus::Completed),
            entry("Tests", acp::PlanEntryStatus::Completed),
            entry("Review", acp::PlanEntryStatus::Cancelled),
        ],
    ));
    harness.run_until(|harness| harness.shows("plan-toggle", "3/3"));
    assert_eq!(harness.bounds("activity-milestone"), Some(settled), "cancellation does not restart the cue");
}

#[test]
fn suppressed_milestones_are_not_replayed_when_activity_becomes_visible_again() {
    for suppression in ["reduced", "inactive", "clipped", "unmounted"] {
        let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
        let (mut harness, thread) =
            DesktopHarnessBuilder::default().agent("/alpha", agent).window_size(900.0, 600.0).start("/alpha");
        let turn = harness.start_turn(thread, &mut requests, "work");
        turn.update(plan("plan", vec![entry("Tests", acp::PlanEntryStatus::InProgress)]));
        harness.run_until_painted("plan-toggle");
        match suppression {
            "reduced" => harness.registry().update(|state| state.reduced_motion = true),
            "inactive" => harness.deactivate(),
            "clipped" => harness.resize(0.0, 0.0),
            _ => harness.press("secondary-2"),
        }
        harness.advance();
        turn.update(plan("plan", vec![entry("Tests", acp::PlanEntryStatus::Completed)]));
        harness.run_until(|_| {
            thread
                .with_core(|core| {
                    core.conversation()
                        .plan()
                        .is_some_and(|plan| plan.entries[0].status == acp::PlanEntryStatus::Completed)
                })
                .unwrap_or(false)
        });
        assert!(harness.bounds("activity-milestone").is_none(), "{suppression} suppresses the cue");
        match suppression {
            "reduced" => harness.registry().update(|state| state.reduced_motion = false),
            "inactive" => harness.activate(),
            "clipped" => harness.resize(900.0, 600.0),
            _ => harness.press("secondary-1"),
        }
        harness.run_until_painted("activity");
        assert!(harness.bounds("activity-milestone").is_none(), "{suppression} never replays a missed cue");
    }
}

#[test]
fn waiting_for_an_answer_is_still_instead_of_looking_busy() {
    let (mut harness, _) = asked(asking_name());
    harness.ui().run(2.0);

    assert!(!harness.ui().is_animating(), "waiting must not keep an animation clock alive");
    assert!(harness.shows("status-label", "Waiting for you"));
}

#[test]
fn inactive_windows_keep_activity_labels_without_running_animation_clocks() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let turn = harness.start_turn(thread, &mut requests, "work");
    turn.update(tool_call("call-1", "Read file", acp::ToolCallStatus::InProgress));
    harness.run_until_painted("activity-working");
    harness.deactivate();
    harness.ui().run(2.0);

    assert!(!harness.ui().is_animating(), "unfocused indicators must not request continuous frames");
    assert!(harness.shows("status-label", "Working"));
    assert!(harness.bounds("tool-wisp-call-1").is_some());

    harness.activate();
    assert!(harness.ui().is_animating(), "working indicators resume on focus");
}

#[test]
fn the_chat_tab_marks_an_agent_at_work_while_another_tab_shows() {
    let (agent, mut requests) = FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) = DesktopHarness::alpha(agent);
    let turn = harness.start_turn(thread, &mut requests, "work");
    assert!(harness.bounds("chat-working").is_none(), "the chat itself shows the work");

    harness.press("secondary-2");
    assert!(harness.bounds("chat-working").is_some(), "the chat tab marks the work");

    turn.end(acp::StopReason::EndTurn);
    harness.run_until(|_| thread.status() == ThreadStatus::Idle);
    assert!(harness.bounds("chat-working").is_none(), "the mark ends with the turn");
}

fn entry(content: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
    acp::PlanEntry::new(content, acp::PlanEntryPriority::Medium, status)
}
