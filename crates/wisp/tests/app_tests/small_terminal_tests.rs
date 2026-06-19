use super::common::*;

#[tokio::test]
async fn test_no_ghost_on_tool_completion_small_terminal() {
    let args = r#"{"file": "a.rs"}"#;
    let renderer = render_with_size(
        vec![tool_call("Read", args), tool_complete("call_Read"), text_chunk("Done"), prompt_done()],
        (80, 8),
    );

    let lines = renderer.writer().get_lines();
    let tool_count = lines.iter().filter(|l| l.contains("Read")).count();
    assert_eq!(tool_count, 1, "Tool name should appear exactly once, got {tool_count}.\nBuffer:\n{}", lines.join("\n"));
}

#[tokio::test]
async fn test_tool_updates_in_place_after_scrollback_push() {
    let renderer = render_with_size(
        vec![
            tool_call_with_id("Read", "call_1", r#"{"file": "a.rs"}"#),
            tool_call_with_id("Write", "call_2", r#"{"file": "b.rs"}"#),
            tool_complete("call_1"),
            text_chunk("Halfway"),
            prompt_done(),
            tool_complete("call_2"),
        ],
        (80, 10),
    );

    let lines = renderer.writer().get_lines();
    let write_count = lines.iter().filter(|l| l.contains("Write")).count();
    assert_eq!(
        write_count,
        1,
        "Write tool should appear exactly once, got {write_count}.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_wrapped_tool_update_does_not_duplicate_lines() {
    let long_args = r#"{"file":"src/some/really/long/path/that/forces/tool/status/wrapping.rs"}"#;
    let renderer =
        render_with_size(vec![tool_call_with_id("Read", "call_1", long_args), tool_complete("call_1")], (40, 12));

    let lines = renderer.writer().get_lines();
    let read_count = lines.iter().filter(|l| l.contains("Read")).count();
    assert_eq!(
        read_count,
        1,
        "Wrapped tool line should update in place, got {read_count} Read rows.\nBuffer:\n{}",
        lines.join("\n")
    );
    assert!(
        lines.iter().any(|l| l.contains("✓")),
        "Completed status should be visible after wrapped update.\nBuffer:\n{}",
        lines.join("\n")
    );
}

#[tokio::test]
async fn test_multiple_scrollback_pushes_tiny_terminal() {
    let renderer = render_with_size(
        vec![
            text_chunk("First message"),
            prompt_done(),
            text_chunk("Second message"),
            prompt_done(),
            text_chunk("Third message"),
            prompt_done(),
        ],
        (80, 8),
    );

    let lines = renderer.writer().get_lines();
    assert!(lines.iter().any(|l| l.contains('>')), "Prompt should be visible.\nBuffer:\n{}", lines.join("\n"));
}

#[tokio::test]
async fn test_prompt_done_does_not_duplicate_overflowed_lines() {
    let markers: Vec<String> = (1..=16).map(|i| format!("L{i:02}")).collect();
    let chunk = format!("```text\n{}\n```", markers.join("\n"));

    let renderer = render_with_size(vec![text_chunk(&chunk), prompt_done()], (40, 8));

    let transcript = renderer.writer().get_transcript_lines();
    for marker in markers.iter().take(8) {
        let count = transcript.iter().filter(|line| line.contains(marker)).count();
        assert_eq!(
            count,
            1,
            "Marker {marker} should appear exactly once in transcript, got {count}.\nTranscript:\n{}",
            transcript.join("\n")
        );
    }
}
