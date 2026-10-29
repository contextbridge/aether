use super::support::*;

#[test]
fn submit_sends_prompt_and_clears_composer() {
    let mut app = TestUi::new();

    app.submit("hello agent");

    let request = sent_prompt(&mut app);
    assert_eq!(request.session_id, SessionId::new("test-session"));
    assert_eq!(request.prompt, vec![acp::ContentBlock::from("hello agent")]);
    assert!(app.app().composer().is_empty());
}

#[test]
fn esc_cancels_only_while_busy() {
    let mut app = TestUi::new();

    app.key(key(KeyCode::Esc));
    assert!(app.agent().cancel.try_recv().is_err());

    app.submit("work");
    app.key(key(KeyCode::Esc));

    assert!(app.agent().cancel.try_recv().is_ok());
}

#[test]
fn double_ctrl_c_exits_and_first_press_clears_composer() {
    let mut app = TestUi::new();
    app.type_text("draft");

    app.key(ctrl('c'));
    assert!(!app.app().exit_requested());
    assert!(app.app().composer().is_empty());
    assert!(app.app().exit_confirmation_active());

    app.key(ctrl('c'));
    assert!(app.app().exit_requested());
}

#[test]
fn ctrl_c_confirmation_disarms_after_window() {
    let mut app = TestUi::new();

    app.key(ctrl('c'));
    app.tick(Instant::now() + Duration::from_secs(2));
    assert!(!app.app().exit_confirmation_active());

    app.key(ctrl('c'));
    assert!(!app.app().exit_requested());
}

#[test]
fn double_ctrl_c_exits_over_session_picker() {
    let mut app = TestUi::new();
    app.open_session_picker(vec![session_info("other", "/tmp/elsewhere", "Other", "2025-01-01T00:00:00Z")]);
    assert_ctrl_c_exits(&mut app);
}

#[test]
fn ctrl_c_during_streaming_arms_exit_without_cancelling() {
    let mut app = TestUi::new();
    app.submit("stream me");
    app.acp_event(text_chunk("partial reply"));
    assert!(app.app().core().waiting_for_response());

    app.key(ctrl('c'));
    assert!(app.app().exit_confirmation_active());
    assert!(!app.app().exit_requested());
    assert!(app.agent().cancel.try_recv().is_err(), "Ctrl+C is exit, not cancel");
    assert!(app.app().core().waiting_for_response());

    app.acp_event(text_chunk("more reply"));
    assert!(app.app().exit_confirmation_active());
    app.key(ctrl('c'));
    assert!(app.app().exit_requested());
}

#[test]
fn double_ctrl_c_exits_over_an_open_completion_overlay() {
    let mut app = TestUi::new();

    // The completion overlay is composer-owned, not a modal Layer.
    app.type_text("/");
    assert!(app.app().composer().has_completion());

    app.key(ctrl('c'));
    assert!(app.app().exit_confirmation_active());
    assert!(!app.app().composer().has_completion());

    app.key(ctrl('c'));
    assert!(app.app().exit_requested());
}

#[test]
fn connection_closed_requests_exit() {
    let mut app = TestUi::new();

    app.acp_event(AcpEvent::ConnectionClosed);

    assert!(app.app().exit_requested());
}

#[test]
fn alt_enter_inserts_newline_instead_of_submitting() {
    let mut app = TestUi::new();
    app.type_text("line one");

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    app.type_text("line two");

    assert!(!sent_any_prompt(&mut app));
    assert_eq!(app.app().composer().text(), "line one\nline two");
}

#[test]
fn context_clear_discards_conversation_retained_in_the_live_viewport() {
    let mut ui = TestUi::new();
    ui.submit("old retained message");
    ui.assert_viewport_contains("old retained message");

    ui.acp_event(context_cleared());

    ui.assert_viewport_not_contains("old retained message");
    ui.assert_viewport_not_contains("[wisp] Context cleared");
}

#[test]
fn fitting_user_message_remains_in_the_live_viewport() {
    let mut ui = TestUi::new();

    ui.submit("hello viewport");

    ui.assert_viewport_contains("hello viewport");
    ui.assert_history_not_contains("hello viewport");
}

#[test]
fn completed_stream_lines_remain_live_until_they_overflow() {
    let mut ui = TestUi::new();
    ui.submit("hi");

    ui.acp_event(text_chunk(&format!("{}partial", numbered_lines("line", 20))));

    ui.assert_history_contains("line-0");
    ui.assert_history_not_contains("partial");
    ui.assert_viewport_contains("line-19");
    ui.assert_viewport_contains("partial");
}

#[test]
fn overflowing_single_paragraph_stream_enters_scrollback_before_completion() {
    let mut ui = TestUi::new();
    ui.submit("stream continuously");

    for index in 0..30 {
        ui.acp_event(text_chunk(&format!("stream-line-{index}\n")));
        ui.draw();
    }
    ui.acp_event(text_chunk("still-streaming"));

    let streaming_history = ui.history_text();
    ui.assert_viewport_not_contains("stream-line-0");
    ui.assert_viewport_contains("still-streaming");
    assert_eq!(streaming_history.matches("stream-line-0").count(), 1, "streaming history:\n{streaming_history}");

    assert_eq!(ui.history_text().matches("stream-line-0").count(), 1, "unchanged redraw duplicated history");

    ui.complete_prompt(acp::StopReason::EndTurn);
    let completed_conversation = ui.conversation_text();
    assert_eq!(
        completed_conversation.matches("stream-line-0").count(),
        1,
        "completed conversation:\n{completed_conversation}"
    );
}

#[test]
fn overflowing_open_fence_enters_scrollback_before_completion() {
    let mut ui = TestUi::new();
    ui.submit("stream a code block");

    ui.acp_event(text_chunk("```text\nfence-start-marker\n"));
    for index in 0..30 {
        ui.acp_event(text_chunk(&format!("fenced-line-{index}\n")));
        ui.draw();
    }
    ui.acp_event(text_chunk("fence-tail-marker"));

    assert_eq!(ui.history_text().matches("fence-start-marker").count(), 1);
    ui.assert_viewport_contains("fence-tail-marker");
}

#[test]
fn stream_after_a_completed_tool_commits_incrementally() {
    let mut ui = TestUi::new();
    ui.submit("use a tool then talk at length");
    ui.acp_event(tool_call("tool-1", "Reading main.rs"));
    ui.acp_event(tool_completed("tool-1"));

    for index in 0..30 {
        ui.acp_event(text_chunk(&format!("after-tool-line-{index}\n\n")));
        ui.draw();
    }

    ui.assert_history_contains("after-tool-line-0");
    ui.assert_viewport_not_contains("after-tool-line-0");
    ui.assert_viewport_contains("after-tool-line-29");

    ui.complete_prompt(acp::StopReason::EndTurn);
    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("after-tool-line-0").count(), 1, "completed conversation:\n{conversation}");
    assert_eq!(conversation.matches("Reading main.rs").count(), 1, "completed conversation:\n{conversation}");
}

#[test]
fn thought_chunks_preserve_identity_without_rendering_in_the_transcript() {
    let mut ui = TestUi::with_dimensions(120, 30);
    ui.submit("think hard");

    for index in 0..30 {
        ui.acp_event(thought_chunk(&format!("pondering-line-{index}\n")));
        ui.draw();
    }

    let thoughts = ui
        .app()
        .core()
        .conversation()
        .items()
        .iter()
        .filter(|item| matches!(item.content(), ConversationContent::Thought(_)));
    assert_eq!(thoughts.count(), 1, "every chunk extends the same thought");
    ui.assert_conversation_not_contains("pondering-line-0");

    ui.assert_viewport_contains("pondering-line-29");
    ui.assert_viewport_contains("Thinking…");
    ui.assert_viewport_contains("esc to interrupt");
}

#[test]
fn partially_committed_assistant_is_not_duplicated_when_a_tool_starts() {
    let mut ui = TestUi::new();
    ui.submit("stream then use a tool");

    ui.acp_event(text_chunk(&format!("assistant-start-marker\n{}", numbered_lines("assistant-line", 30))));
    ui.assert_history_contains("assistant-start-marker");

    ui.acp_event(tool_call("tool-1", "Reading after stream"));

    assert_eq!(ui.conversation_text().matches("assistant-start-marker").count(), 1);
    ui.assert_viewport_contains("Reading after stream");
    ui.assert_history_not_contains("Reading after stream");
}

#[test]
fn resizing_a_partially_committed_stream_keeps_new_content_visible() {
    let mut ui = TestUi::with_dimensions(20, 15);
    ui.submit("stream across resize");

    for index in 0..30 {
        ui.acp_event(text_chunk(&format!("resize-line-{index}-abcdefghij\n")));
    }
    ui.assert_history_contains("resize-line-0");

    ui.resize(60, 15);
    ui.acp_event(text_chunk("after-resize"));

    ui.assert_viewport_contains("after-resize");

    ui.complete_prompt(acp::StopReason::EndTurn);
    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("resize-line-0").count(), 1, "completed conversation:\n{conversation}");
    assert_eq!(conversation.matches("after-resize").count(), 1, "completed conversation:\n{conversation}");
}

#[test]
fn completed_streaming_text_remains_adjacent_to_the_composer_once() {
    let mut ui = TestUi::new();
    ui.submit("hi");

    ui.acp_event(text_chunk("streamed "));
    ui.acp_event(text_chunk("answer"));

    ui.assert_viewport_contains("streamed answer");
    ui.assert_history_not_contains("streamed answer");

    ui.complete_prompt(acp::StopReason::EndTurn);

    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("streamed answer").count(), 1);
    ui.assert_viewport_contains("streamed answer");
}

#[test]
fn streamed_markdown_blocks_keep_blank_separators() {
    let mut ui = TestUi::with_dimensions(80, 30);
    ui.submit("format this");

    for chunk in ["### Root\n", "\n", "Keep this paragraph separate.\n", "\n", "- first item\n"] {
        ui.acp_event(text_chunk(chunk));
    }
    ui.complete_prompt(acp::StopReason::EndTurn);

    let conversation = ui.conversation();
    let heading = row_containing(&conversation, "### Root").expect("heading should render");
    let paragraph = row_containing(&conversation, "Keep this paragraph separate.").expect("paragraph should render");
    let item = row_containing(&conversation, "first item").expect("list item should render");

    assert_eq!(paragraph - heading, 2, "heading and paragraph need one blank row");
    assert_eq!(item - paragraph, 2, "paragraph and list need one blank row");
}

#[test]
fn running_tool_holds_later_content_out_of_committed_history() {
    let mut ui = TestUi::new();
    ui.submit("run a tool");

    ui.acp_event(tool_call("tool-1", "Reading main.rs"));
    ui.acp_event(text_chunk("tool output summary"));

    ui.assert_viewport_contains("Reading main.rs");
    ui.assert_history_not_contains("Reading main.rs");

    ui.acp_event(tool_completed("tool-1"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.assert_conversation_contains("Reading main.rs");
    ui.assert_conversation_contains("tool output summary");
    assert!(ui.app().core().conversation().items().iter().all(|item| item.state() == ItemState::Sealed));
}

#[test]
fn a_tool_with_a_multi_line_title_shows_only_its_first_line() {
    let mut ui = TestUi::new();
    ui.submit("run a script");

    ui.acp_event(tool_call("tool-1", "\nset -e\ncargo build\n"));

    ui.assert_viewport_contains("set -e");
    ui.assert_viewport_not_contains("cargo build");
}

#[test]
fn cancelled_prompt_marks_running_tool_as_error() {
    let mut app = TestUi::new();
    app.submit("run a tool");
    app.acp_event(tool_call("tool-1", "Slow tool"));

    app.complete_prompt(acp::StopReason::Cancelled);

    let cancelled =
        app.app().core().conversation().items().iter().any(
            |item| matches!(item.content(), ConversationContent::Tool(tool) if tool.status == ToolStatus::Cancelled),
        );
    assert!(cancelled, "expected cancelled tool in {:?}", app.app().core().conversation().items());
}

fn tool_call_with_raw(id: &str, title: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(id.to_string()).title(title).raw_input(raw_input),
    ))
}

fn tool_call_update_with_raw(id: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(id.to_string()).raw_input(raw_input)))
}

#[test]
fn raw_input_updates_replace_and_show_after_completion() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Edit file"));
    ui.acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("first ".to_string())));
    ui.acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("second".to_string())));
    ui.acp_event(tool_completed("tool-1"));

    ui.assert_viewport_contains("second");
    ui.assert_viewport_not_contains("first");
}

fn tool_call_update_with_display_value(id: &str, display_value: &str) -> AcpEvent {
    let mut meta = serde_json::Map::new();
    meta.insert("display_value".to_string(), serde_json::Value::String(display_value.to_string()));
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(id.to_string()).meta(meta)))
}

#[test]
fn completed_bash_tool_renders_the_command_with_shell_syntax_highlighting() {
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run shell command");
    let tool = acp::ToolCallUpdate::new("bash-1")
        .title("Bash")
        .raw_input(serde_json::json!({"command": "if true; then echo $HOME; fi", "description": "Check shell syntax"}))
        .name("coding__bash");
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(tool)));
    ui.acp_event(tool_completed("bash-1"));

    assert_shell_command_highlighted(&mut ui, "Bash", "if true; then echo $HOME; fi");
    ui.assert_conversation_not_contains("description");
}

#[test]
fn completed_bash_tool_shows_its_command_once_followed_by_the_exit_status() {
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run shell command");
    ran_bash_tool(&mut ui, "cargo test", "cargo test (exit 2)");

    let viewport = ui.viewport_text();
    assert!(viewport.contains("Ran cargo test (exit 2)"), "{viewport}");
    assert_eq!(viewport.matches("cargo test").count(), 1, "the summary must not repeat the command: {viewport}");
}

#[test]
fn completed_multiline_bash_tool_shows_the_full_command_instead_of_its_truncated_summary() {
    let command = "python3 - <<'PY'\nfrom pathlib import Path\nprint(Path.cwd())\nPY";
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run shell command");
    ran_bash_tool(&mut ui, command, "python3 - <<'PY' from pathlib import ... (exit 0)");

    let viewport = ui.viewport_text();
    assert!(viewport.contains("Ran python3 - <<'PY' (exit 0)"), "{viewport}");
    assert!(command.lines().all(|line| viewport.contains(line)), "every command line should render: {viewport}");
    assert!(!viewport.contains("..."), "the truncated summary must not render: {viewport}");
}

fn ran_bash_tool(ui: &mut TestUi, command: &str, display_value: &str) {
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new("bash-1")
            .title("Bash")
            .name("coding__bash")
            .raw_input(serde_json::json!({ "command": command })),
    )));
    ui.acp_event(tool_call_update_with_display_value("bash-1", display_value));
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new("bash-1").title("Ran").status(acp::ToolCallStatus::Completed),
    )));
}

#[test]
fn non_bash_tool_with_a_command_argument_keeps_generic_rendering() {
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run other command tool");
    let tool = acp::ToolCallUpdate::new("other-1")
        .title("Execute")
        .raw_input(serde_json::json!({"command": "if true; then echo no; fi"}))
        .name("other__execute");
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(tool)));
    ui.acp_event(tool_completed("other-1"));

    ui.assert_viewport_contains(r#"{"command":"if true; then echo no; fi"}"#);
    assert!(!has_cell(&ui.conversation(), "i", |cell| cell.bg == ui.app().theme().code_bg));
}

#[test]
fn running_tool_hides_raw_arguments() {
    let mut ui = TestUi::new();
    ui.submit("run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));

    ui.assert_viewport_contains("Read file");
    ui.assert_viewport_not_contains("/src/main.rs");
}

#[test]
fn completed_tool_shows_raw_arguments() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    ui.acp_event(tool_completed("tool-1"));

    ui.assert_viewport_contains("/src/main.rs");
}

#[test]
fn display_value_overrides_raw_arguments_in_rendered_output() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    ui.acp_event(tool_call_update_with_display_value("tool-1", "42 lines read"));
    ui.acp_event(tool_completed("tool-1"));

    ui.assert_viewport_contains("42 lines read");
    ui.assert_viewport_not_contains("/src/main.rs");
}

#[test]
fn error_cause_is_visible_in_rendered_output() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Failing tool"));
    ui.acp_event(tool_status("tool-1", acp::ToolCallStatus::Failed));

    ui.assert_viewport_contains("failed");
    ui.assert_viewport_not_contains("(failed)");
}

#[test]
fn completed_tool_arguments_truncate_at_200_characters_not_bytes() {
    let cases: [(String, &[&str], &[&str]); 5] = [
        ("hello world".to_string(), &["hello world"], &["…"]),
        ("x".repeat(199), &[], &["…"]),
        ("x".repeat(300), &["…"], &[]),
        (format!("{}{}", "a".repeat(100), "界".repeat(50)), &["界"], &["…"]),
        (format!("{}こんにちは世界", "a".repeat(195)), &["…", "ん"], &["ち"]),
    ];
    for (argument, shown, hidden) in cases {
        let mut ui = TestUi::with_dimensions(250, 15);
        ui.submit("run tool");
        ui.acp_event(tool_call_with_raw("tool-1", "Arg tool", serde_json::Value::String(argument.clone())));
        ui.acp_event(tool_completed("tool-1"));

        let viewport = ui.viewport();
        let row = row_text(&viewport, row_containing(&viewport, "Arg tool").expect("tool row"));
        for text in shown {
            assert!(row.contains(text), "{argument:?} should show {text:?}: {row}");
        }
        for text in hidden {
            assert!(!row.contains(text), "{argument:?} should not show {text:?}: {row}");
        }
    }
}

#[test]
fn truncation_preserved_in_scrollback() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    let long = "x".repeat(300);
    ui.acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    ui.acp_event(tool_completed("tool-1"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.assert_conversation_contains("…");
}

#[test]
fn tool_arguments_preserved_in_scrollback_exactly_once() {
    let mut ui = TestUi::new();
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.submit("run tool");
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    ui.acp_event(tool_completed("tool-1"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    let conversation = ui.conversation_text();
    let occurrences = conversation.matches("/src/main.rs").count();
    assert_eq!(occurrences, 1, "tool args must appear exactly once in conversation: {conversation}");
    assert!(conversation.contains("Read file"), "title must appear: {conversation}");
}

#[test]
fn diff_not_rendered_while_tool_is_running() {
    let mut ui = TestUi::new();
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::patch(
        "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old content\n+new content\n",
        vec![acp::DiffChange::modify(acp::AbsolutePath::new("/src/main.rs"))],
    );
    let update = acp::ToolCallUpdate::new("tool-1").content(vec![acp::ToolCallContent::Diff(diff)]);
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));

    ui.assert_viewport_contains("Edit file");
    ui.assert_viewport_not_contains("old content");
    ui.assert_viewport_not_contains("new content");
}

#[test]
fn diff_not_rendered_after_failed_status() {
    let mut ui = TestUi::new();
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::patch(
        "diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old content\n+new content\n",
        vec![acp::DiffChange::modify(acp::AbsolutePath::new("/src/main.rs"))],
    );
    let update = acp::ToolCallUpdate::new("tool-1")
        .content(vec![acp::ToolCallContent::Diff(diff)])
        .status(acp::ToolCallStatus::Failed);
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.assert_conversation_not_contains("old content");
    ui.assert_conversation_not_contains("new content");
    ui.assert_conversation_contains("failed");
}

#[test]
fn short_streaming_message_renders_above_progress_band_and_composer() {
    let mut ui = TestUi::new();
    ui.submit("prompt");
    ui.acp_event(text_chunk("short answer"));

    let prompt_row = ui.viewport_row("prompt").unwrap();
    let message_row = ui.viewport_row("short answer").unwrap();
    let spinner_row = ui.viewport_row("⠋").unwrap();
    let composer_row = ui.viewport_row("> ").unwrap();
    assert!(prompt_row < message_row);
    assert!(
        message_row < spinner_row && spinner_row < composer_row,
        "the progress band must sit between the message and the composer \
         (message {message_row}, spinner {spinner_row}, composer {composer_row})"
    );
}

#[test]
fn composer_echo_and_status_line_render_in_viewport() {
    let mut ui = TestUi::new();

    ui.type_text("typing");

    ui.assert_viewport_contains("> typing");
    ui.assert_viewport_contains("~/code/demo · main");
    ui.assert_viewport_contains("aether");
}
