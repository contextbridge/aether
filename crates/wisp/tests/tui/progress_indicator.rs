use wisp::conversation::progress_indicator::SPINNER_FRAMES;

use super::support::*;

fn progress_ui() -> (TestUi, Instant) {
    let now = Instant::now();
    let mut ui = status_ui();
    ui.tick(now);
    ui.submit("hello");
    (ui, now)
}

fn activity_row(ui: &mut TestUi, label: &str) -> String {
    let viewport = ui.viewport();
    let row = row_containing(&viewport, label).unwrap_or_else(|| panic!("activity row {label:?} not found"));
    row_text(&viewport, row)
}

#[test]
fn keeps_status_segments_visible_until_prompt_completes() {
    let (mut ui, _) = progress_ui();
    ui.acp_event(context_usage(100_000, 200_000));

    for expected in ["Thinking…", "esc to interrupt", "Claude Sonnet", "ctx", "~/code/demo"] {
        ui.assert_viewport_contains(expected);
    }

    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.assert_viewport_not_contains("esc to interrupt");
    ui.assert_viewport_contains("Claude Sonnet");
}

#[test]
fn native_idle_clears_responding_state() {
    let (mut ui, _) = progress_ui();
    ui.acp_event(text_chunk("final answer"));
    ui.assert_viewport_contains("Responding…");

    ui.acp_event(session_update(
        acp_utils::testing::idle_notification("test-session", Some(acp::StopReason::EndTurn)).update,
    ));

    assert!(!ui.app().waiting_for_response());
    assert!(!ui.app().is_agent_busy());
    ui.assert_viewport_not_contains("Responding…");
}

#[test]
fn updates_after_prompt_completion_do_not_restart_progress() {
    let (mut ui, _) = progress_ui();
    ui.acp_event(text_chunk("final answer"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.acp_event(text_chunk("late chunk"));
    ui.acp_event(thought_chunk("late thought"));
    ui.acp_event(tool_call("late-tool", "Late tool"));
    ui.acp_event(compaction_update("compaction", acp::CompactionStatus::InProgress));

    assert!(
        ui.app().conversation_items().iter().any(|item| item.text().is_some_and(|text| text.contains("late chunk")))
    );
    ui.assert_viewport_not_contains("late thought");
    ui.assert_viewport_not_contains("Responding…");
    ui.assert_viewport_not_contains("Working…");
    ui.assert_viewport_not_contains("Thinking…");
    ui.assert_viewport_not_contains("Compacting context...");
    assert!(!ui.app().progress_indicator().is_active());
    assert!(!ui.app().waiting_for_response());
}

#[test]
fn elapsed_resets_when_the_phase_changes() {
    let (mut ui, t0) = progress_ui();
    assert!(activity_row(&mut ui, "Thinking…").contains("0s"));

    ui.tick(t0 + Duration::from_secs(64));
    assert!(activity_row(&mut ui, "Thinking…").contains("1m04s"));

    ui.acp_event(text_chunk("answering"));
    assert!(activity_row(&mut ui, "Responding…").contains("0s"));

    ui.tick(t0 + Duration::from_secs(69));
    assert!(activity_row(&mut ui, "Responding…").contains("5s"));
}

#[test]
fn reasoning_is_ephemeral_and_scoped_to_thinking() {
    let (mut ui, t0) = progress_ui();
    ui.acp_event(thought_chunk("first\n thought"));
    ui.assert_viewport_contains("first thought");

    ui.tick(t0 + Duration::from_millis(100));
    let viewport = ui.viewport();
    let label = row_containing(&viewport, "Thinking…").expect("activity row");
    let thought = row_containing(&viewport, "first thought").expect("reasoning preview");
    let composer = row_containing(&viewport, "> ").expect("composer row");
    assert_eq!(label, thought, "reasoning preview should share the activity row");
    assert!(thought < composer);

    ui.acp_event(text_chunk("answering"));
    ui.assert_viewport_not_contains("first thought");
    ui.acp_event(thought_chunk_with_id("fresh", "fresh reasoning"));
    ui.tick(t0 + Duration::from_millis(200));
    ui.assert_viewport_contains("fresh reasoning");
    ui.assert_viewport_not_contains("first thought");

    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.submit("again");
    ui.tick(t0 + Duration::from_millis(300));
    ui.assert_viewport_not_contains("fresh reasoning");
}

#[test]
fn thought_upserts_replace_and_clear_only_the_ephemeral_preview() {
    let (mut ui, _) = progress_ui();
    for (content, expected) in [
        (serde_json::json!([{"type": "text", "text": "initial reasoning"}]), "initial reasoning"),
        (serde_json::json!([{"type": "text", "text": "replacement reasoning"}]), "replacement reasoning"),
        (serde_json::Value::Null, ""),
    ] {
        ui.acp_event(session_update(
            serde_json::from_value(serde_json::json!({
                "sessionUpdate": "agent_thought", "messageId": "thought-upsert", "content": content
            }))
            .unwrap(),
        ));
        if expected.is_empty() {
            ui.assert_viewport_not_contains("reasoning");
        } else {
            assert!(activity_row(&mut ui, "Thinking…").contains(expected));
        }
        ui.assert_history_not_contains("reasoning");
    }
    ui.assert_viewport_not_contains("reasoning");
    ui.acp_event(session_update(acp::SessionUpdate::AgentThoughtChunk(acp::ContentChunk::new(
        "appended reasoning".into(),
        "thought-upsert",
    ))));
    ui.acp_event(session_update(
        serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought", "messageId": "thought-upsert"
        }))
        .unwrap(),
    ));
    ui.acp_event(session_update(
        serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought", "messageId": "another-thought", "content": null
        }))
        .unwrap(),
    ));
    assert!(activity_row(&mut ui, "Thinking…").contains("appended reasoning"));
    ui.acp_event(session_update(
        serde_json::from_value(serde_json::json!({
            "sessionUpdate": "agent_thought", "messageId": "thought-upsert", "content": []
        }))
        .unwrap(),
    ));
    ui.assert_viewport_not_contains("reasoning");
}

#[test]
fn hidden_agent_transition_discards_stale_reasoning() {
    let (mut ui, _) = progress_ui();
    ui.acp_event(compaction_update("compaction", acp::CompactionStatus::InProgress));
    ui.acp_event(thought_chunk("stale reasoning"));
    ui.acp_event(text_chunk("answering"));
    ui.acp_event(compaction_update("compaction", acp::CompactionStatus::Completed));
    ui.acp_event(thought_chunk_with_id("fresh", "fresh reasoning"));

    ui.assert_viewport_contains("fresh reasoning");
    ui.assert_viewport_not_contains("stale reasoning");
}

#[test]
fn working_phase_spans_consecutive_tools() {
    let (mut ui, t0) = progress_ui();
    ui.acp_event(tool_call("tool-1", "Read src/app/mod.rs"));
    ui.tick(t0 + Duration::from_secs(3));
    let row = activity_row(&mut ui, "Working…");
    assert!(!row.contains("Read src/app/mod.rs") && row.contains("3s"), "{row}");

    ui.acp_event(tool_completed("tool-1"));
    ui.acp_event(tool_call("tool-2", "Run cargo test"));
    assert!(activity_row(&mut ui, "Working…").contains("3s"));

    ui.tick(t0 + Duration::from_secs(9));
    let row = activity_row(&mut ui, "Working…");
    assert!(!row.contains("Run cargo test") && row.contains("9s"), "{row}");
}

#[test]
fn short_layouts_clip_gracefully() {
    for height in 2..=8 {
        let mut ui = TestUiBuilder::new().dimensions(120, height).build();
        ui.submit("hello");
        ui.draw();
        if height >= 7 {
            ui.assert_viewport_contains("Thinking…");
        }
    }
}

#[test]
fn phases_follow_agent_events() {
    let (mut ui, _) = progress_ui();
    for (event, expected) in [
        (thought_chunk("pondering"), "Thinking…"),
        (text_chunk("answering"), "Responding…"),
        (tool_call("tool-1", "Edit file"), "Working…"),
    ] {
        ui.acp_event(event);
        ui.assert_viewport_contains(expected);
    }
}

#[test]
fn stray_thought_chunk_is_not_interruptible() {
    let mut ui = TestUiBuilder::new().dimensions(120, 15).build();
    ui.acp_event(thought_chunk("pondering"));
    ui.assert_viewport_not_contains("Thinking…");
    ui.assert_viewport_not_contains("pondering");
    ui.assert_viewport_not_contains("esc to interrupt");
}

/// What the progress band shows once `setup` has run.
struct Scenario {
    name: &'static str,
    setup: Step,
    shown: &'static [&'static str],
    hidden: &'static [&'static str],
}

#[test]
fn progress_band_shows_while_work_is_in_flight() {
    assert_progress(
        true,
        &[
            Scenario {
                name: "prompt in flight",
                setup: |ui| ui.submit("hello"),
                shown: &["esc to interrupt"],
                hidden: &[],
            },
            Scenario {
                name: "compaction",
                setup: |ui| ui.acp_event(compacting()),
                shown: &["Compacting context"],
                hidden: &[],
            },
            Scenario {
                name: "compaction takes precedence over agent work",
                setup: |ui| {
                    ui.submit("hello");
                    ui.acp_event(compacting());
                },
                shown: &["Compacting context", "esc to interrupt"],
                hidden: &["Thinking…"],
            },
            Scenario {
                name: "workspace move",
                setup: move_to_other_workspace,
                shown: &["Moving workspace"],
                hidden: &["esc to interrupt"],
            },
            Scenario {
                name: "workspace move takes precedence over compaction",
                setup: |ui| {
                    move_to_other_workspace(ui);
                    ui.acp_event(compacting());
                },
                shown: &["Moving workspace"],
                hidden: &["Compacting"],
            },
            Scenario {
                name: "loading the moved session",
                setup: |ui| {
                    move_to_other_workspace(ui);
                    ui.deliver_result(workspace_moved("/home/user/code/other"));
                },
                shown: &["Loading session in new workspace"],
                hidden: &[],
            },
        ],
    );
}

#[test]
fn progress_band_clears_once_work_ends() {
    assert_progress(
        false,
        &[
            Scenario { name: "idle", setup: |_| {}, shown: &[], hidden: &["Working…", "esc to interrupt"] },
            Scenario {
                name: "completed prompt",
                setup: |ui| {
                    ui.submit("hello");
                    ui.tick(Instant::now());
                    ui.acp_event(text_chunk("response"));
                    ui.complete_prompt(acp::StopReason::EndTurn);
                    ui.tick(Instant::now());
                },
                shown: &[],
                hidden: &["esc to interrupt", "Working…"],
            },
            Scenario {
                name: "finished compaction",
                setup: |ui| {
                    ui.acp_event(compacting());
                    ui.acp_event(compaction_update("compaction", acp::CompactionStatus::Completed));
                },
                shown: &[],
                hidden: &["Compacting context"],
            },
            Scenario {
                name: "failed workspace move",
                setup: |ui| {
                    move_to_other_workspace(ui);
                    ui.deliver_result(workspace_move_failed("permission denied"));
                },
                shown: &[],
                hidden: &["Moving workspace", "Loading session"],
            },
            Scenario {
                name: "failed workspace listing",
                setup: |ui| {
                    ui.run_command("/move");
                    ui.deliver_result(workspace_list_failed("network error"));
                },
                shown: &[],
                hidden: &["Moving workspace"],
            },
            Scenario {
                name: "context cleared",
                setup: |ui| {
                    compacting_prompt(ui);
                    ui.acp_event(context_cleared());
                },
                shown: &[],
                hidden: &["Compacting", "esc to interrupt"],
            },
            Scenario {
                name: "new session",
                setup: |ui| {
                    compacting_prompt(ui);
                    ui.complete_prompt(acp::StopReason::EndTurn);
                    ui.run_command("/clear");
                    ui.deliver_result(new_session_created("new-session", Vec::new()));
                },
                shown: &[],
                hidden: &["Compacting"],
            },
            Scenario {
                name: "resumed session",
                setup: |ui| {
                    compacting_prompt(ui);
                    ui.complete_prompt(acp::StopReason::EndTurn);
                    ui.begin_resume("loaded-session", "/tmp");
                    ui.deliver_result(session_loaded("loaded-session", Vec::new()));
                },
                shown: &[],
                hidden: &["Compacting"],
            },
        ],
    );
}

#[test]
fn tick_animates_the_spinner() {
    let mut ui = TestUi::with_dimensions(120, 30);
    ui.submit("hello");
    let before = ui.viewport_text();

    ui.tick(Instant::now());

    assert_ne!(ui.viewport_text(), before, "spinner should animate with each tick");
}

#[test]
fn progress_is_not_inserted_into_scrollback() {
    let mut ui = TestUi::new();
    ui.turn("hello", &numbered_lines("line", 20));

    ui.submit("another");

    let scrollback = ui.history_text();
    assert!(!SPINNER_FRAMES.iter().any(|frame| scrollback.contains(frame)), "spinner in scrollback:\n{scrollback}");
    assert!(!scrollback.contains("esc to interrupt"), "esc hint in scrollback:\n{scrollback}");
    assert!(!scrollback.contains("Working…"), "progress label in scrollback:\n{scrollback}");
}

fn assert_progress(ticking: bool, scenarios: &[Scenario]) {
    for Scenario { name, setup, shown, hidden } in scenarios {
        let mut ui = TestUiBuilder::new().workspace_move().dimensions(120, 30).build();
        setup(&mut ui);

        let viewport = ui.viewport_text();
        let spinning = SPINNER_FRAMES.iter().any(|frame| viewport.contains(frame));
        assert_eq!(spinning, ticking, "{name}: spinner visibility\n{viewport}");
        assert_eq!(ui.app().wants_tick(), ticking, "{name}: wants_tick");
        for text in *shown {
            assert!(viewport.contains(text), "{name}: {text:?} should show\n{viewport}");
        }
        for text in *hidden {
            assert!(!viewport.contains(text), "{name}: {text:?} should not show\n{viewport}");
        }
    }
}

fn compacting() -> AcpEvent {
    compaction_update("compaction", acp::CompactionStatus::InProgress)
}

fn compacting_prompt(ui: &mut TestUi) {
    ui.submit("hello");
    ui.acp_event(compacting());
}
