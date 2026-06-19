use tui::testing::assert_buffer_eq;

use super::common::*;

#[tokio::test]
async fn test_tool_calls_interleave_with_thought_and_text_in_arrival_order() {
    let renderer = render(vec![thought_chunk("Thinking"), tool_call("search", r#"{"q":"rust"}"#), text_chunk("Done")]);

    let expected = expected_with_prompt(
        &[&p("Thinking"), "", &p("⠒ search"), "", &p("Done"), "", &p(PROGRESS_LINE), ""],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_agent_message_tool_call() {
    let renderer = render(vec![tool_call("test_tool", r#"{"arg1": "value1"}"#)]);

    let expected = expected_with_prompt(&[&p("⠒ test_tool"), "", &p(PROGRESS_LINE), ""], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_agent_message_tool_result() {
    let args = r#"{"arg1": "value1"}"#;
    let renderer = render(vec![tool_call("test_tool", args), tool_complete("call_test_tool")]);

    let expected = expected_with_prompt(&[&p(r#"✓ test_tool {"arg1":"value1"}"#)], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_multiple_messages_sequence() {
    let args = r#"{"query": "test"}"#;
    let renderer = render(vec![
        text_chunk("Processing your request"),
        prompt_done(),
        tool_call("search", args),
        tool_complete("call_search"),
        text_chunk("Found results"),
        prompt_done(),
    ]);

    let expected = expected_with_prompt(
        &[&p("Processing your request"), "", &p(r#"✓ search {"query":"test"}"#), "", &p("Found results")],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_streaming_tool_call_arguments() {
    let renderer = render(vec![
        tool_call_with_id("Read", "call_1", ""),
        tool_update_with_args("call_1", r#"{"file":"test.rs"}"#),
        tool_complete("call_1"),
    ]);

    let expected = expected_with_prompt(&[&p(r#"✓ Read {"file":"test.rs"}"#)], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_in_progress_tool_call_updates_from_duplicate_requests() {
    let renderer =
        render(vec![tool_call_with_id("Read", "call_1", ""), tool_call_with_id("", "call_1", r#"{"file":"test.rs"}"#)]);

    let expected = expected_with_prompt(&[&p("⠒ Read"), "", &p(PROGRESS_LINE), ""], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_tool_progress_renders_running_tool() {
    let renderer = render(vec![tool_call_with_id("Read", "call_1", r#"{"file":"test.rs"}"#)]);

    let expected = expected_with_prompt(&[&p("⠒ Read"), "", &p(PROGRESS_LINE), ""], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_multiple_parallel_tool_calls() {
    let args1 = r#"{"file": "test.rs"}"#;
    let args2 = r#"{"pattern": "foo"}"#;
    let args3 = r#"{"path": "src/"}"#;

    let renderer = render(vec![
        tool_call("Read", args1),
        tool_call("Grep", args2),
        tool_call("Glob", args3),
        tool_complete("call_Read"),
        tool_complete("call_Grep"),
        tool_complete("call_Glob"),
    ]);

    let expected = expected_with_prompt(
        &[&p(r#"✓ Read {"file":"test.rs"}"#), &p(r#"✓ Grep {"pattern":"foo"}"#), &p(r#"✓ Glob {"path":"src/"}"#)],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_prompt_done_finalizes_running_tool_calls() {
    let renderer = render(vec![
        tool_call_with_id("Read", "call_1", r#"{"file": "a.rs"}"#),
        tool_call_with_id("Write", "call_2", r#"{"file": "b.rs"}"#),
        tool_complete("call_1"),
        text_chunk("Done reading"),
        prompt_done(),
    ]);

    let expected = expected_with_prompt(
        &[&p(r#"✓ Read {"file":"a.rs"}"#), &p(r#"✓ Write {"file":"b.rs"}"#), "", &p("Done reading")],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_late_result_after_prompt_done() {
    let renderer = render(vec![
        tool_call_with_id("Read", "call_1", r#"{"file": "a.rs"}"#),
        tool_call_with_id("Write", "call_2", r#"{"file": "b.rs"}"#),
        tool_complete("call_1"),
        text_chunk("Done reading"),
        prompt_done(),
        tool_complete("call_2"),
    ]);

    let expected = expected_with_prompt(
        &[&p(r#"✓ Read {"file":"a.rs"}"#), &p(r#"✓ Write {"file":"b.rs"}"#), "", &p("Done reading")],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_tool_complete_with_display_meta_shows_display_value() {
    let renderer = render(vec![
        tool_call_with_id("read_file", "call_1", r#"{"filePath":"/Users/josh/code/aether/Cargo.toml"}"#),
        tool_complete_with_display_meta(
            "call_1",
            &serde_json::json!({
                "title": "Read file",
                "value": "Cargo.toml, 156 lines"
            }),
        ),
    ]);

    let expected = expected_with_prompt(&[&p("✓ Read file (Cargo.toml, 156 lines)")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_tool_complete_without_display_meta_shows_raw_args() {
    let args = r#"{"filePath":"/Users/josh/code/aether/Cargo.toml"}"#;
    let renderer = render(vec![tool_call_with_id("read_file", "call_1", args), tool_complete("call_1")]);

    let expected = expected_with_prompt(
        &[&p(r#"✓ read_file {"filePath":"/Users/josh/code/aether/Cargo.toml"}"#)],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_running_tool_hides_raw_args() {
    let renderer = render(vec![tool_call_with_id("read_file", "call_1", r#"{"filePath":"Cargo.toml"}"#)]);

    let lines = renderer.writer().get_lines();
    let tool_line = lines.iter().find(|l| l.contains("read_file")).unwrap();
    assert!(!tool_line.contains("filePath"), "Running tool should hide raw args: {tool_line}");
    assert_eq!(tool_line.trim(), "⠒ read_file", "Running tool should show only name: {tool_line}");
}

#[tokio::test]
async fn test_display_meta_title_overrides_tool_name() {
    let renderer = render(vec![
        tool_call_with_id("coding__read_file", "call_1", r#"{"filePath":"main.rs"}"#),
        tool_complete_with_display_meta(
            "call_1",
            &serde_json::json!({
                "title": "Read file",
                "value": "main.rs, 42 lines"
            }),
        ),
    ]);

    let lines = renderer.writer().get_lines();
    let tool_line = lines.iter().find(|l| l.contains("✓")).unwrap();
    assert!(tool_line.contains("Read file"), "Display title should override raw tool name: {tool_line}");
    assert!(tool_line.contains("(main.rs, 42 lines)"), "Display value should appear in parens: {tool_line}");
}

#[tokio::test]
async fn test_multiple_tools_with_mixed_display_meta() {
    let renderer = render(vec![
        tool_call_with_id("read_file", "call_1", r#"{"filePath":"Cargo.toml"}"#),
        tool_call_with_id("external_tool", "call_2", r#"{"key":"value"}"#),
        tool_complete_with_display_meta(
            "call_1",
            &serde_json::json!({
                "title": "Read file",
                "value": "Cargo.toml, 156 lines"
            }),
        ),
        tool_complete("call_2"),
    ]);

    let expected = expected_with_prompt(
        &[&p("✓ Read file (Cargo.toml, 156 lines)"), &p(r#"✓ external_tool {"key":"value"}"#)],
        TEST_WIDTH,
        "",
        TEST_AGENT,
    );
    assert_buffer_eq(renderer.writer(), &expected);
}

#[tokio::test]
async fn test_command_display_meta_shows_exit_code() {
    let renderer = render(vec![
        tool_call_with_id("bash", "call_1", r#"{"command":"cargo test"}"#),
        tool_complete_with_display_meta(
            "call_1",
            &serde_json::json!({
                "title": "Run command",
                "value": "cargo test (exit 0)"
            }),
        ),
    ]);

    let expected = expected_with_prompt(&[&p("✓ Run command (cargo test (exit 0))")], TEST_WIDTH, "", TEST_AGENT);
    assert_buffer_eq(renderer.writer(), &expected);
}
