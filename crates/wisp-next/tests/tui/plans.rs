use super::support::*;

fn plan_entry(content: &str, status: acp::PlanEntryStatus) -> acp::PlanEntry {
    acp::PlanEntry::new(content.to_string(), acp::PlanEntryPriority::Medium, status)
}

fn plan_update(entries: Vec<acp::PlanEntry>) -> AcpEvent {
    session_update(acp::SessionUpdate::Plan(acp::Plan::new(entries)))
}

#[test]
fn plan_renders_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_tall();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![
        plan_entry("Research", acp::PlanEntryStatus::InProgress),
        plan_entry("Implement", acp::PlanEntryStatus::Pending),
    ]));

    assert!(app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Plan"), "viewport should show Plan header:\n{viewport}");
    assert!(viewport.contains("Research"), "viewport should show Research:\n{viewport}");
    assert!(viewport.contains("Implement"), "viewport should show Implement:\n{viewport}");
}

#[test]
fn plan_ordering_in_viewport() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_tall();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![
        plan_entry("Completed task", acp::PlanEntryStatus::Completed),
        plan_entry("Pending task", acp::PlanEntryStatus::Pending),
        plan_entry("InProgress task", acp::PlanEntryStatus::InProgress),
    ]));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    let in_progress_pos = viewport.find("InProgress task").unwrap();
    let pending_pos = viewport.find("Pending task").unwrap();
    let completed_pos = viewport.find("Completed task").unwrap();

    assert!(in_progress_pos < pending_pos, "InProgress should render before Pending:\n{viewport}");
    assert!(pending_pos < completed_pos, "Pending should render before Completed:\n{viewport}");
}

#[test]
fn plan_grace_period_hides_completed() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    let now = Instant::now();

    app.plan_tracker_mut().replace(vec![plan_entry("Done", acp::PlanEntryStatus::Completed)], now);
    app.plan_tracker_mut().on_tick(now);

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(viewport.contains("Done"), "completed entry visible at t=0:\n{viewport}");

    app.plan_tracker_mut().on_tick(now + Duration::from_secs(5));
    app.on_key(key(KeyCode::Enter));
    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport_after = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport_after.contains("Done"), "completed entry hidden after 5s grace:\n{viewport_after}");
}

#[test]
fn plan_grace_period_timestamp_preserved_across_repeated_updates() {
    let (mut app, _command_rx) = make_app();

    let now = Instant::now();
    app.plan_tracker_mut().replace(vec![plan_entry("Task", acp::PlanEntryStatus::Completed)], now);

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Completed)]));

    app.plan_tracker_mut().on_tick(now);
    assert!(app.has_plan(), "still visible at original completion time");

    app.plan_tracker_mut().on_tick(now + Duration::from_secs(10));
    let entries = app.plan_entries();
    assert!(entries.is_empty(), "hidden when original timestamp exceeds grace");
}

#[test]
fn plan_coexists_with_streaming_transcript() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Research", acp::PlanEntryStatus::InProgress)]));

    submit_prompt(&mut app, "explain");
    app.on_acp_event(text_chunk("Here is the explanation."));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Plan"), "plan header visible:\n{viewport}");
    assert!(viewport.contains("Research"), "plan entry visible:\n{viewport}");
    assert!(viewport.contains("Here is the explanation."), "transcript visible:\n{viewport}");
}

#[test]
fn plan_coexists_with_tool_calls() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Edit files", acp::PlanEntryStatus::InProgress)]));

    submit_prompt(&mut app, "fix it");
    app.on_acp_event(tool_call("tool-1", "Editing main.rs"));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Edit files"), "plan entry visible:\n{viewport}");
    assert!(viewport.contains("Editing main.rs"), "tool call visible:\n{viewport}");
}

#[test]
fn plan_short_terminal_clips_plan() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal_with_width(40);
    terminal.backend_mut().resize(40, 8);
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![
        plan_entry("Plan item one", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item two", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item three", acp::PlanEntryStatus::Pending),
        plan_entry("Plan item four", acp::PlanEntryStatus::Pending),
    ]));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));

    assert!(viewport.contains("Plan"), "plan header visible on short terminal:\n{viewport}");
    let visible_entries = vec!["Plan item one", "Plan item two", "Plan item three", "Plan item four"]
        .into_iter()
        .filter(|e| viewport.contains(e))
        .count();
    assert!(visible_entries < 4, "short terminal should clip plan, but all 4 entries visible:\n{viewport}");
}

#[test]
fn plan_not_in_scrollback() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("A plan task", acp::PlanEntryStatus::InProgress)]));

    submit_prompt(&mut app, "hello");
    app.on_acp_event(text_chunk("response text"));
    app.on_acp_event(AcpEvent::PromptDone(acp::StopReason::EndTurn));

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let scrollback = buffer_text(&history_buffer(&mut terminal));

    assert!(!scrollback.contains("A plan task"), "plan should not be in scrollback:\n{scrollback}");
}

#[test]
fn plan_cleared_on_context_cleared() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert!(app.has_plan());

    app.on_acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));
    assert!(!app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Task"), "plan should be gone after context clear:\n{viewport}");
}

#[test]
fn plan_cleared_on_new_session() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert!(app.has_plan());

    app.on_acp_event(AcpEvent::NewSessionCreated { session_id: SessionId::new("new-id"), config_options: Vec::new() });
    assert!(!app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Task"), "plan should be gone after new session:\n{viewport}");
}

#[test]
fn plan_cleared_on_session_loaded() {
    let (mut app, _command_rx) = make_app();
    let mut terminal = make_terminal();
    let mut renderer = TranscriptRenderer::new(&UiSettings::default());

    app.on_acp_event(plan_update(vec![plan_entry("Task", acp::PlanEntryStatus::Pending)]));
    assert!(app.has_plan());

    app.on_acp_event(AcpEvent::SessionLoaded {
        session_id: SessionId::new("other-session"),
        config_options: Vec::new(),
    });
    assert!(!app.has_plan());

    sync_terminal_with_renderer(&mut terminal, &mut app, &mut renderer).unwrap();
    let viewport = buffer_text(&viewport_buffer(&mut terminal));
    assert!(!viewport.contains("Task"), "plan should be gone after session load:\n{viewport}");
}

// --- MCP Server Status & Provider Login Tests ---
