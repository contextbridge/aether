use super::support::*;

fn plan_entry(content: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
    acp::PlanEntry::new(content.to_string(), acp::PlanEntryPriority::Medium, status)
}

fn plan_update(entries: Vec<acp::PlanEntry>) -> AcpEvent {
    session_update(plan("plan", entries))
}

#[test]
fn plan_renders_in_viewport() {
    let mut ui = TestUi::with_dimensions(80, 30);
    ui.acp_event(plan_update(vec![
        plan_entry("Research", acp::PlanEntryStatus::InProgress),
        plan_entry("Implement", acp::PlanEntryStatus::Pending),
    ]));

    assert_eq!(ui.app().plan_entries().len(), 2);

    ui.assert_viewport_contains("Plan");
    ui.assert_viewport_contains("Research");
    ui.assert_viewport_contains("Implement");
}

#[test]
fn plan_ordering_in_viewport() {
    let mut ui = TestUi::with_dimensions(80, 30);
    ui.acp_event(plan_update(vec![
        plan_entry("Completed task", acp::PlanEntryStatus::Completed),
        plan_entry("Pending task", acp::PlanEntryStatus::Pending),
        plan_entry("InProgress task", acp::PlanEntryStatus::InProgress),
    ]));

    let viewport = ui.viewport_text();

    let in_progress_pos = viewport.find("InProgress task").unwrap();
    let pending_pos = viewport.find("Pending task").unwrap();
    let completed_pos = viewport.find("Completed task").unwrap();

    assert!(in_progress_pos < pending_pos, "InProgress should render before Pending:\n{viewport}");
    assert!(pending_pos < completed_pos, "Pending should render before Completed:\n{viewport}");
}

#[test]
fn plan_grace_period_hides_completed() {
    let mut ui = TestUi::new();
    let now = Instant::now();

    ui.acp_event(plan_update(vec![plan_entry("Done", acp::PlanEntryStatus::Completed)]));

    ui.assert_viewport_contains("Done");

    ui.tick(now + Duration::from_secs(5));
    ui.assert_viewport_not_contains("Done");
}

#[test]
fn plan_grace_period_timestamp_preserved_across_repeated_updates() {
    let mut ui = TestUi::new();

    let now = Instant::now();
    ui.acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Completed)]));

    ui.acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Completed)]));

    ui.tick(now);
    assert_eq!(ui.app().plan_entries().len(), 1, "still visible at original completion time");

    ui.tick(now + Duration::from_secs(10));
    let entries = ui.app().plan_entries();
    assert!(entries.is_empty(), "hidden when original timestamp exceeds grace");
}

#[test]
fn plan_coexists_with_streaming_transcript() {
    let mut ui = TestUi::new();
    ui.acp_event(plan_update(vec![plan_entry("Research", acp::PlanEntryStatus::InProgress)]));

    ui.submit("explain");
    ui.acp_event(text_chunk("Here is the explanation."));

    let viewport = ui.viewport();
    let transcript_row = row_containing(&viewport, "Here is the explanation.").expect("transcript visible");
    let plan_row = row_containing(&viewport, "Plan").expect("plan header visible");
    let viewport = buffer_text(&viewport);

    assert!(viewport.contains("Plan"), "plan header visible:\n{viewport}");
    assert!(viewport.contains("Research"), "plan entry visible:\n{viewport}");
    assert!(viewport.contains("Here is the explanation."), "transcript visible:\n{viewport}");
    assert!(
        transcript_row < plan_row,
        "transcript should render above plan (transcript row {transcript_row}, plan row {plan_row}):\n{viewport}"
    );
}

#[test]
fn plan_coexists_with_tool_calls() {
    let mut ui = TestUi::new();
    ui.acp_event(plan_update(vec![plan_entry("Edit files", acp::PlanEntryStatus::InProgress)]));

    ui.submit("fix it");
    ui.acp_event(tool_call("tool-1", "Editing main.rs"));

    ui.assert_viewport_contains("Edit files");
    ui.assert_viewport_contains("Editing main.rs");
}

#[test]
fn plan_short_terminal_clips_plan() {
    let mut ui = TestUi::new();
    ui.resize(40, 8);

    ui.acp_event(plan_update(vec![
        plan_entry("Plan item one", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item two", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item three", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item four", acp::PlanEntryStatus::Pending),
    ]));

    let viewport = ui.viewport_text();

    assert!(viewport.contains("Plan"), "plan header visible on short terminal:\n{viewport}");
    let visible_entries = vec!["Plan item one", "Plan item two", "Plan item three", "Plan item four"]
        .into_iter()
        .filter(|e| viewport.contains(e))
        .count();
    assert!(visible_entries < 4, "short terminal should clip plan, but all 4 entries visible:\n{viewport}");
}

#[test]
fn plan_not_in_scrollback() {
    let mut ui = TestUi::new();
    ui.acp_event(plan_update(vec![plan_entry("A plan task", acp::PlanEntryStatus::InProgress)]));

    ui.turn("hello", "response text");

    ui.assert_history_not_contains("A plan task");
}

#[test]
fn plan_cleared_on_context_cleared() {
    let mut ui = TestUi::new();
    ui.acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert_eq!(ui.app().plan_entries().len(), 1);

    ui.acp_event(context_cleared());
    assert_eq!(ui.app().plan_entries(), []);

    ui.assert_viewport_not_contains("Task");
}

#[test]
fn plan_cleared_on_new_session() {
    let mut ui = TestUi::new();
    ui.acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert_eq!(ui.app().plan_entries().len(), 1);

    ui.run_command("/clear");
    new_session_created(&mut ui, "new-id", Vec::new());
    assert_eq!(ui.app().plan_entries(), []);

    ui.assert_viewport_not_contains("Task");
}

#[test]
fn plan_cleared_on_session_loaded() {
    let mut ui = TestUi::new();
    ui.acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert_eq!(ui.app().plan_entries().len(), 1);

    ui.begin_resume("other-session", "/tmp");
    session_loaded(&mut ui, Vec::new());
    assert_eq!(ui.app().plan_entries(), []);

    ui.assert_viewport_not_contains("Task");
}
