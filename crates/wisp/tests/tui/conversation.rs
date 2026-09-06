use super::support::*;
use unicode_width::UnicodeWidthStr;

#[test]
fn submit_sends_prompt_and_clears_composer() {
    let mut app = make_app();

    app.submit("hello agent");

    match app.next_agent_command().unwrap() {
        AgentCommand::Prompt { session_id, text, content } => {
            assert_eq!(session_id.0.as_ref(), "test-session");
            assert_eq!(text, "hello agent");
            assert!(content.is_none());
        }
        other => panic!("expected Prompt command, got {other:?}"),
    }
    assert!(app.app().composer().is_empty());
}

#[test]
fn submit_is_ignored_while_prompt_in_flight() {
    let mut app = make_app();
    app.submit("first");
    app.next_agent_command().unwrap();

    app.submit("second");

    assert!(app.next_command().is_none());
    assert_eq!(app.app().composer().text(), "second");
}

#[test]
fn esc_cancels_only_while_busy() {
    let mut app = make_app();

    app.key(key(KeyCode::Esc));
    assert!(app.next_command().is_none());

    app.submit("work");
    app.next_agent_command().unwrap();
    app.key(key(KeyCode::Esc));

    assert!(matches!(app.next_agent_command().unwrap(), AgentCommand::Cancel { .. }));
}

#[test]
fn double_ctrl_c_exits_and_first_press_clears_composer() {
    let mut app = make_app();
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
    let mut app = make_app();

    app.key(ctrl('c'));
    app.tick(Instant::now() + Duration::from_secs(2));
    assert!(!app.app().exit_confirmation_active());

    app.key(ctrl('c'));
    assert!(!app.app().exit_requested());
}

#[test]
fn double_ctrl_c_exits_over_session_picker() {
    let mut app = make_app();
    app.type_text("/resume");
    app.key(key(KeyCode::Tab));
    let _ = app.next_agent_command().unwrap();
    app.deliver_result(sessions_listed(vec![session_info("other", "/tmp/elsewhere", "Other", "2025-01-01T00:00:00Z")]));
    assert_ctrl_c_exits(&mut app);
}

#[test]
fn ctrl_c_during_streaming_arms_exit_without_cancelling() {
    let mut app = make_app();
    app.submit("stream me");
    app.next_agent_command().unwrap();
    app.acp_event(text_chunk("partial reply"));
    assert!(app.app().waiting_for_response());

    app.key(ctrl('c'));
    assert!(app.app().exit_confirmation_active());
    assert!(!app.app().exit_requested());
    assert!(app.next_command().is_none(), "Ctrl+C is exit, not cancel");
    assert!(app.app().waiting_for_response());

    app.acp_event(text_chunk("more reply"));
    assert!(app.app().exit_confirmation_active());
    app.key(ctrl('c'));
    assert!(app.app().exit_requested());
}

#[test]
fn double_ctrl_c_exits_over_an_open_completion_overlay() {
    let mut app = make_app();

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
    let mut app = make_app();

    app.acp_event(AcpEvent::ConnectionClosed);

    assert!(app.app().exit_requested());
}

#[test]
fn alt_enter_inserts_newline_instead_of_submitting() {
    let mut app = make_app();
    app.type_text("line one");

    app.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
    app.type_text("line two");

    assert!(app.next_command().is_none());
    assert_eq!(app.app().composer().text(), "line one\nline two");
}

#[test]
fn context_clear_discards_conversation_retained_in_the_live_viewport() {
    let mut ui = TestUi::new();
    ui.submit("old retained message");
    ui.draw();
    ui.assert_viewport_contains("old retained message");

    ui.acp_event(AcpEvent::ContextCleared(ContextClearedParams::default()));
    ui.draw();

    let viewport = ui.viewport_text();
    assert!(!viewport.contains("old retained message"), "{viewport}");
    assert!(!viewport.contains("[wisp] Context cleared"), "{viewport}");
}

#[test]
fn fitting_user_message_remains_in_the_live_viewport() {
    let mut ui = TestUi::new();

    ui.submit("hello viewport");
    ui.draw();

    ui.assert_viewport_contains("hello viewport");
    ui.assert_history_not_contains("hello viewport");
}

#[test]
fn completed_stream_lines_remain_live_until_they_overflow() {
    let mut ui = TestUi::new();
    ui.submit("hi");

    let mut completed = String::new();
    for index in 0..20 {
        writeln!(completed, "line-{index}\n").unwrap();
    }
    ui.acp_event(text_chunk(&format!("{completed}partial")));
    ui.draw();

    let scrollback = ui.history_text();
    let viewport = ui.viewport_text();
    assert!(scrollback.contains("line-0"));
    assert!(!scrollback.contains("partial"));
    assert!(viewport.contains("line-19"));
    assert!(viewport.contains("partial"));
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
    ui.draw();

    let streaming_history = ui.history_text();
    ui.assert_viewport_not_contains("stream-line-0");
    ui.assert_viewport_contains("still-streaming");
    assert_eq!(streaming_history.matches("stream-line-0").count(), 1, "streaming history:\n{streaming_history}");

    ui.draw();
    assert_eq!(ui.history_text().matches("stream-line-0").count(), 1, "unchanged redraw duplicated history");

    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.draw();
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
    ui.draw();

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
    ui.draw();
    let conversation = ui.conversation_text();
    assert_eq!(conversation.matches("after-tool-line-0").count(), 1, "completed conversation:\n{conversation}");
    assert_eq!(conversation.matches("Reading main.rs").count(), 1, "completed conversation:\n{conversation}");
}

#[test]
fn thought_chunks_drive_progress_activity_without_appending() {
    let mut ui = TestUi::with_dimensions(120, 30);
    ui.submit("think hard");

    for index in 0..30 {
        ui.acp_event(thought_chunk(&format!("pondering-line-{index}\n")));
        ui.draw();
    }

    assert!(
        ui.app()
            .conversation_items()
            .iter()
            .filter_map(|item| item.text())
            .all(|text| !text.contains("pondering-line")),
        "thought chunks must not append conversation items"
    );
    assert_eq!(ui.history_text().matches("pondering-line-0").count(), 0, "thought text must not reach scrollback");

    let status = ui.viewport_text();
    assert!(status.contains("pondering-line-29"), "the progress band should preview the latest thought:\n{status}");
    assert!(status.contains("Thinking…"), "thought chunks should drive the progress-band activity, got:\n{status}");
    assert!(status.contains("esc to interrupt"), "{status}");
}

#[test]
fn partially_committed_assistant_is_not_duplicated_when_a_tool_starts() {
    let mut ui = TestUi::new();
    ui.submit("stream then use a tool");

    let mut reply = String::from("assistant-start-marker\n");
    for index in 0..30 {
        writeln!(reply, "assistant-line-{index}").unwrap();
    }
    ui.acp_event(text_chunk(&reply));
    ui.draw();
    ui.assert_history_contains("assistant-start-marker");

    ui.acp_event(tool_call("tool-1", "Reading after stream"));
    ui.draw();

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
    ui.draw();
    ui.assert_history_contains("resize-line-0");

    ui.resize(60, 15);
    ui.acp_event(text_chunk("after-resize"));
    ui.draw();

    ui.assert_viewport_contains("after-resize");

    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.draw();
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
    ui.draw();

    ui.assert_viewport_contains("streamed answer");
    ui.assert_history_not_contains("streamed answer");

    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.draw();
    ui.draw();

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
    ui.draw();

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
    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("Reading main.rs"));
    assert!(!ui.history_text().contains("Reading main.rs"));

    ui.acp_event(tool_completed("tool-1"));
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.draw();

    let conversation = ui.conversation_text();
    assert!(conversation.contains("Reading main.rs"));
    assert!(conversation.contains("tool output summary"));
    assert!(ui.app().conversation_items().iter().all(|item| item.state() == ItemState::Sealed));
}

#[test]
fn cancelled_prompt_marks_running_tool_as_error() {
    let mut app = make_app();
    app.submit("run a tool");
    app.acp_event(tool_call("tool-1", "Slow tool"));

    app.complete_prompt(acp::StopReason::Cancelled);

    let cancelled = app.app().conversation_items().iter().any(|item| {
        matches!(item.content(), ConversationContent::Tool(tool) if tool.status == ToolStatus::Error("cancelled".to_string()))
    });
    assert!(cancelled, "expected cancelled tool in {:?}", app.app().conversation_items());
}

fn tool_call_with_raw(id: &str, title: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCall(acp::ToolCall::new(id.to_string(), title).raw_input(raw_input)))
}

fn tool_call_update_with_raw(id: &str, raw_input: serde_json::Value) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().raw_input(raw_input),
    )))
}

#[test]
fn streamed_raw_input_fragments_accumulate_and_show_after_completion() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Edit file"));
    ui.acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("first ".to_string())));
    ui.acp_event(tool_call_update_with_raw("tool-1", serde_json::Value::String("second".to_string())));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("first second"), "streamed fragments must appear: {viewport}");
}

fn tool_call_update_with_display_value(id: &str, display_value: &str) -> AcpEvent {
    let mut meta = serde_json::Map::new();
    meta.insert("display_value".to_string(), serde_json::Value::String(display_value.to_string()));
    session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(id.to_string(), acp::ToolCallUpdateFields::new()).meta(meta),
    ))
}

fn tool_completed_status(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Completed),
    )))
}

fn tool_failed_status(id: &str) -> AcpEvent {
    session_update(acp::SessionUpdate::ToolCallUpdate(acp::ToolCallUpdate::new(
        id.to_string(),
        acp::ToolCallUpdateFields::new().status(acp::ToolCallStatus::Failed),
    )))
}

#[test]
fn completed_bash_tool_renders_the_command_with_shell_syntax_highlighting() {
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run shell command");
    let mut meta = serde_json::Map::new();
    meta.insert(acp_utils::AETHER_TOOL_NAME_META_KEY.to_string(), "coding__bash".into());
    let tool = acp::ToolCall::new("bash-1".to_string(), "Bash")
        .raw_input(serde_json::json!({"command": "if true; then echo $HOME; fi", "description": "Check shell syntax"}))
        .meta(meta);
    ui.acp_event(session_update(acp::SessionUpdate::ToolCall(tool)));
    ui.acp_event(tool_completed_status("bash-1"));

    ui.draw();

    let conversation = ui.conversation();
    let command = "if true; then echo $HOME; fi";
    let row = row_containing(&conversation, command).expect("rendered Bash command");
    let text = row_text(&conversation, row);
    assert!(text.contains(&format!("Bash {command}")), "command should share the tool row: {text:?}");
    let command_start = u16::try_from(text[..text.find(command).expect("command position")].width()).unwrap();
    let gap = conversation.cell((conversation.area.left() + command_start - 1, row)).expect("gap before command");
    assert_eq!(gap.bg, Color::Reset, "the gap should not set a background");
    let cells = (command_start..command_start + u16::try_from(command.width()).unwrap())
        .filter_map(|offset| conversation.cell((conversation.area.left() + offset, row)))
        .filter(|cell| cell.symbol() != " ")
        .collect::<Vec<_>>();
    assert!(cells.iter().all(|cell| cell.bg == Color::Reset), "command should sit on the terminal background");
    let keyword = cells.iter().find(|cell| cell.symbol() == "i").expect("if keyword");
    let variable = cells.iter().find(|cell| cell.symbol() == "$").expect("shell variable");
    assert_ne!(keyword.fg, variable.fg, "shell keywords and variables should use distinct token colors");
    let text = buffer_text(&conversation);
    assert!(!text.contains("description"), "only the command field should be rendered as shell code: {text}");
}

#[test]
fn bash_tool_keeps_highlighting_after_title_and_display_metadata_updates() {
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run shell command");
    let mut tool_meta = serde_json::Map::new();
    tool_meta.insert(acp_utils::AETHER_TOOL_NAME_META_KEY.to_string(), "coding__bash".into());
    let command = "cargo test";
    let tool = acp::ToolCall::new("bash-1".to_string(), "Bash")
        .raw_input(serde_json::json!({"command": command}))
        .meta(tool_meta);
    ui.acp_event(session_update(acp::SessionUpdate::ToolCall(tool)));
    let mut update_meta = serde_json::Map::new();
    update_meta.insert("display_value".to_string(), format!("{command} (exit 0)").into());
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(
        acp::ToolCallUpdate::new(
            "bash-1".to_string(),
            acp::ToolCallUpdateFields::new().title("Ran").status(acp::ToolCallStatus::Completed),
        )
        .meta(update_meta),
    )));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(
        viewport.contains("Ran cargo test (exit 0)") && !viewport.contains("Run command"),
        "a finished bash call should read as completed: {viewport}"
    );
    assert_eq!(viewport.matches(command).count(), 1, "the command should render exactly once: {viewport}");
    assert!(!has_cell(&ui.conversation(), "c", |cell| cell.bg == ui.app().theme().code_bg));
}

#[test]
fn non_bash_tool_with_a_command_argument_keeps_generic_rendering() {
    let mut ui = TestUi::with_dimensions(100, 15);
    ui.submit("run other command tool");
    let mut meta = serde_json::Map::new();
    meta.insert(acp_utils::AETHER_TOOL_NAME_META_KEY.to_string(), "other__execute".into());
    let tool = acp::ToolCall::new("other-1".to_string(), "Execute")
        .raw_input(serde_json::json!({"command": "if true; then echo no; fi"}))
        .meta(meta);
    ui.acp_event(session_update(acp::SessionUpdate::ToolCall(tool)));
    ui.acp_event(tool_completed_status("other-1"));

    ui.draw();

    assert!(ui.viewport_text().contains(r#"{"command":"if true; then echo no; fi"}"#));
    assert!(!has_cell(&ui.conversation(), "i", |cell| cell.bg == ui.app().theme().code_bg));
}

#[test]
fn running_tool_hides_raw_arguments() {
    let mut ui = TestUi::new();
    ui.submit("run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("Read file"), "title must be visible: {viewport}");
    assert!(!viewport.contains("/src/main.rs"), "raw args must be hidden while running: {viewport}");
}

#[test]
fn completed_tool_shows_raw_arguments() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("/src/main.rs"), "raw args must be visible after completion: {viewport}");
}

#[test]
fn display_value_overrides_raw_arguments_in_rendered_output() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    ui.acp_event(tool_call_update_with_display_value("tool-1", "42 lines read"));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("42 lines read"), "display_value must be visible: {viewport}");
    assert!(!viewport.contains("/src/main.rs"), "raw args must be hidden when display_value is set: {viewport}");
}

#[test]
fn error_cause_is_visible_in_rendered_output() {
    let mut ui = TestUi::with_dimensions(80, 15);
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Failing tool"));
    ui.acp_event(tool_failed_status("tool-1"));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("failed"), "error cause must be visible: {viewport}");
    assert!(!viewport.contains("(failed)"), "error cause must NOT have parentheses: {viewport}");
}

#[test]
fn truncation_adds_visible_ellipsis_for_long_arguments() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    let long = "x".repeat(300);
    ui.acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let buffer = ui.viewport();
    let arg_row = row_containing(&buffer, "Long arg").expect("long arg row");
    assert!(row_text(&buffer, arg_row).contains('…'), "truncated args must show ellipsis");
}

#[test]
fn truncation_keeps_short_arguments_unchanged() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    let short = "hello world";
    ui.acp_event(tool_call_with_raw("tool-1", "Short arg", serde_json::Value::String(short.to_string())));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let buffer = ui.viewport();
    let arg_row = row_containing(&buffer, short).expect("short arg row");
    assert!(row_text(&buffer, arg_row).contains(short), "short args must appear in full");
    assert!(!row_text(&buffer, arg_row).contains('…'), "short args must NOT have ellipsis");
}

#[test]
fn truncation_is_unicode_safe_no_split_characters() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    let prefix = "a".repeat(195);
    let unicode_arg = format!("{prefix}こんにちは世界");
    ui.acp_event(tool_call_with_raw("tool-1", "Unicode tool", serde_json::Value::String(unicode_arg)));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let buffer = ui.viewport();
    let arg_row = row_containing(&buffer, "Unicode tool").expect("unicode arg row");
    let row = row_text(&buffer, arg_row);
    assert!(row.contains('…'), "truncated Unicode args must show ellipsis");
    assert!(row.contains('ん'), "char-based truncation must preserve leading multi-byte chars");
    assert!(!row.contains('ち'), "truncation must stop at char boundary (199 chars)");
}

#[test]
fn char_based_truncation_preserves_multi_byte_under_200_chars() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    // 100 ASCII chars + 50 × '界' (3 bytes each) = 150 chars, but 100 + 150 = 250 bytes
    // This is under 200 chars so it must NOT be truncated even though it exceeds 200 bytes.
    let half = "a".repeat(100);
    let unicode_half = "界".repeat(50);
    let arg = format!("{half}{unicode_half}");
    assert!(arg.len() > 200, "arg must exceed 200 bytes for this test");
    assert!(arg.chars().count() < 200, "arg must be under 200 chars (with leading space)");
    ui.acp_event(tool_call_with_raw("tool-1", "Unicode tool", serde_json::Value::String(arg)));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let buffer = ui.viewport();
    let arg_row = row_containing(&buffer, "界").expect("arg row");
    let row = row_text(&buffer, arg_row);
    assert!(!row.contains('…'), "under 200 chars must not be truncated");
    assert!(row.contains('界'), "multi-byte chars must appear in full");
}

#[test]
fn truncation_boundary_exactly_200_bytes_no_ellipsis() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    let exactly_199 = "x".repeat(199);
    ui.acp_event(tool_call_with_raw("tool-1", "199-char arg", serde_json::Value::String(exactly_199)));
    ui.acp_event(tool_completed_status("tool-1"));

    ui.draw();

    let buffer = ui.viewport();
    let arg_row = row_containing(&buffer, "199-char arg").expect("199-char arg row");
    assert!(!row_text(&buffer, arg_row).contains('…'), "199-char args must not be truncated (fit in 200 with space)");
}

#[test]
fn truncation_preserved_in_scrollback() {
    let mut ui = TestUi::with_dimensions(250, 15);
    ui.submit("run tool");
    let long = "x".repeat(300);
    ui.acp_event(tool_call_with_raw("tool-1", "Long arg", serde_json::Value::String(long)));
    ui.acp_event(tool_completed_status("tool-1"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.draw();
    ui.draw();

    let conversation = ui.conversation_text();
    assert!(conversation.contains('…'), "truncation must survive drain to scrollback: {conversation}");
}

#[test]
fn tool_arguments_preserved_in_scrollback_exactly_once() {
    let mut ui = TestUi::new();
    let raw = serde_json::json!({"path": "/src/main.rs"});
    ui.submit("run tool");
    ui.acp_event(tool_call_with_raw("tool-1", "Read file", raw));
    ui.acp_event(tool_completed_status("tool-1"));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.draw();
    ui.draw();

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

    let diff = acp::Diff::new("src/main.rs", "new content").old_text("old content");
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new().content(vec![acp::ToolCallContent::Diff(diff)]),
    );
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));

    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("Edit file"), "title must be visible: {viewport}");
    assert!(!viewport.contains("old content"), "diff must NOT render while running: {viewport}");
    assert!(!viewport.contains("new content"), "diff must NOT render while running: {viewport}");
}

#[test]
fn diff_not_rendered_after_failed_status() {
    let mut ui = TestUi::new();
    ui.submit("run tool");
    ui.acp_event(tool_call("tool-1", "Edit file"));

    let diff = acp::Diff::new("src/main.rs", "new content").old_text("old content");
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new()
            .content(vec![acp::ToolCallContent::Diff(diff)])
            .status(acp::ToolCallStatus::Failed),
    );
    ui.acp_event(session_update(acp::SessionUpdate::ToolCallUpdate(update)));
    ui.complete_prompt(acp::StopReason::EndTurn);

    ui.draw();
    ui.draw();

    let conversation = ui.conversation_text();
    assert!(!conversation.contains("old content"), "diff must NOT render after failure: {conversation}");
    assert!(!conversation.contains("new content"), "diff must NOT render after failure: {conversation}");
    assert!(conversation.contains("failed"), "error cause must be visible: {conversation}");
}

#[test]
fn short_streaming_message_renders_above_progress_band_and_composer() {
    let mut ui = TestUi::new();
    ui.submit("prompt");
    ui.acp_event(text_chunk("short answer"));

    ui.draw();

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
    ui.draw();

    let viewport = ui.viewport_text();
    assert!(viewport.contains("> typing"));
    assert!(viewport.contains("~/code/demo · main"));
    assert!(viewport.contains("aether"));
}
