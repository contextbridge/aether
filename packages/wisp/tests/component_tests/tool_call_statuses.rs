use super::support::tool_events::*;
use agent_client_protocol::schema as acp;
use serde_json::json;
use tui::testing::{assert_buffer_eq, render_component, render_lines};
use tui::{BRAILLE_FRAMES as FRAMES, DiffLine, DiffPreview, DiffTag, Line, SplitDiffCell, SplitDiffRow, ViewContext};
use wisp::components::tool_call_status_view::{MAX_TOOL_ARG_LENGTH, ToolCallStatus, ToolCallStatusView};
use wisp::components::tool_call_statuses::ToolCallStatuses;

fn ctx() -> ViewContext {
    ViewContext::new((80, 24))
}

fn render_all(statuses: &ToolCallStatuses, ids: &[&str], ctx: &ViewContext) -> Vec<Line> {
    ids.iter().flat_map(|id| statuses.render_tool(id, ctx).into_lines()).collect()
}

fn render_tool_lines(statuses: &ToolCallStatuses, id: &str) -> Vec<String> {
    let lines = statuses.render_tool(id, &ctx()).into_lines();
    let count = lines.len();
    let term = render_lines(&lines, 80, 24);
    term.get_lines().into_iter().take(count).collect()
}

fn setup_parent() -> ToolCallStatuses {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("parent-1").name("spawn_subagent").build());
    statuses
}

fn progress(statuses: &mut ToolCallStatuses, agent: &str, event: SubAgentEvent) {
    statuses.on_sub_agent_progress(&sub_agent_progress("parent-1", agent, event));
}

#[test]
fn request_tracks_tool() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(
        &ToolCallFactory::default().id("tool-1").name("Read").raw_input_json(r#""/path/to/file""#).build(),
    );

    let output = render_tool_lines(&statuses, "tool-1");
    assert_eq!(output.len(), 1);
    assert!(output[0].contains("Read"));
}

#[test]
fn update_to_success() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-1").name("Read").build());
    statuses.on_tool_call_update(&completed_tool_call_update("tool-1"));

    let output = render_tool_lines(&statuses, "tool-1");
    assert_eq!(output.len(), 1);
    assert!(output[0].contains("✓"));
}

#[test]
fn unknown_update_is_ignored() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call_update(&completed_tool_call_update("unknown"));
    assert!(statuses.render_tool("unknown", &ctx()).lines().is_empty());
}

#[test]
fn update_to_error() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-1").name("Read").build());
    statuses.on_tool_call_update(&failed_tool_call_update("tool-1"));

    let output = render_tool_lines(&statuses, "tool-1");
    assert_eq!(output.len(), 1);
    assert!(output[0].contains("✗"));
}

#[test]
fn multiple_tools_render_in_order() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-1").name("Read").build());
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-2").name("Write").build());

    let lines = render_all(&statuses, &["tool-1", "tool-2"], &ctx());
    assert_eq!(lines.len(), 2);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("Read"));
    assert!(output[1].contains("Write"));
}

#[test]
fn multiple_tools_complete_independently() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-1").name("Read").build());
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-2").name("Write").build());
    statuses.on_tool_call_update(&completed_tool_call_update("tool-1"));

    let lines = render_all(&statuses, &["tool-1", "tool-2"], &ctx());
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("✓")); // Read completed
    assert!(!output[1].contains("✓")); // Write still running
}

#[test]
fn clear_removes_all() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-1").name("Read").build());
    statuses.clear();
    assert!(!statuses.has_tool("tool-1"));
    assert!(statuses.render_tool("tool-1", &ctx()).lines().is_empty());
}

#[test]
fn view_renders_running_with_spinner() {
    let status = ToolCallStatus::Running;
    let view = ToolCallStatusView { arguments: "test args", ..tool_call_status_view(&status) };
    let lines = view.render(&ctx()).into_lines();
    assert_eq!(lines.len(), 1);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("TestTool"));
    assert!(!output[0].contains("test args"));
    assert!(output[0].contains(FRAMES[0]));
}

#[test]
fn view_running_spinner_changes_with_tick() {
    let status = ToolCallStatus::Running;
    let view_a = tool_call_status_view(&status);
    let view_b = ToolCallStatusView { tick: 1, ..tool_call_status_view(&status) };
    let lines_a = view_a.render(&ctx()).into_lines();
    let lines_b = view_b.render(&ctx()).into_lines();
    let term_a = render_lines(&lines_a, 80, 24);
    let term_b = render_lines(&lines_b, 80, 24);
    let a = &term_a.get_lines()[0];
    let b = &term_b.get_lines()[0];
    assert_ne!(a, b);
}

#[test]
fn view_renders_success() {
    let status = ToolCallStatus::Success;
    let view = ToolCallStatusView { arguments: "test args", ..tool_call_status_view(&status) };
    let lines = view.render(&ctx()).into_lines();
    assert_eq!(lines.len(), 1);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("✓"));
}

#[test]
fn view_renders_error() {
    let status = ToolCallStatus::Error("boom".to_string());
    let view = ToolCallStatusView { arguments: "test args", ..tool_call_status_view(&status) };
    let lines = view.render(&ctx()).into_lines();
    assert_eq!(lines.len(), 1);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(output[0].contains("✗"));
    assert!(output[0].contains("boom"));
}

#[test]
fn view_truncates_utf8_arguments_without_panicking() {
    let arguments = format!("{}界", "a".repeat(MAX_TOOL_ARG_LENGTH - 2));
    let status = ToolCallStatus::Success;
    let view = ToolCallStatusView { arguments: &arguments, ..tool_call_status_view(&status) };

    let expected = format!("✓ TestTool {}", "a".repeat(MAX_TOOL_ARG_LENGTH - 2));
    let width = expected.len() + 10;
    // width chosen so the long argument fits on one row; we're testing
    // utf-8 boundary handling, not wrapping.
    #[allow(clippy::cast_possible_truncation)]
    let non_wrapping_ctx = ViewContext::new((width as u16, 24));
    let lines = view.render(&non_wrapping_ctx).into_lines();
    assert_eq!(lines.len(), 1);
    #[allow(clippy::cast_possible_truncation)]
    let term = render_lines(&lines, width as u16, 24);
    let output = term.get_lines();
    assert_eq!(output[0], expected);
}

#[test]
fn view_running_hides_raw_args_then_shows_display_value() {
    let status = ToolCallStatus::Running;
    let view = ToolCallStatusView {
        name: "Read",
        arguments: r#"{"file_path":"/path/to/main.rs"}"#,
        ..tool_call_status_view(&status)
    };

    // While running with no display_value, raw args are hidden
    let lines = view.render(&ctx()).into_lines();
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert!(!output[0].contains("file_path"));
    assert_eq!(output[0], format!("{} Read", FRAMES[0]));

    // After display_value arrives, it is shown
    let view = ToolCallStatusView { display_value: Some("main.rs"), ..view };
    let lines = view.render(&ctx()).into_lines();
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    assert_eq!(output[0], format!("{} Read (main.rs)", FRAMES[0]));
}

#[test]
fn indented_split_diff_does_not_bleed_diff_bg_into_left_indent_columns() {
    // conversation_window indents tool-call frames; the indent columns must
    // stay neutral instead of inheriting the diff bg from later spans.
    let preview = DiffPreview {
        lines: vec![
            DiffLine { tag: DiffTag::Removed, content: "old code".to_string() },
            DiffLine { tag: DiffTag::Added, content: "new code".to_string() },
        ],
        rows: vec![SplitDiffRow {
            left: Some(SplitDiffCell { tag: DiffTag::Removed, content: "old code".to_string(), line_number: Some(1) }),
            right: Some(SplitDiffCell { tag: DiffTag::Added, content: "new code".to_string(), line_number: Some(1) }),
        }],
        lang_hint: String::new(),
        start_line: None,
    };
    let status = ToolCallStatus::Success;
    let view = ToolCallStatusView {
        name: "Edit",
        display_value: Some("file.rs"),
        diff_preview: Some(&preview),
        ..tool_call_status_view(&status)
    };

    let indent: u16 = 2;
    let term = render_component(|ctx| view.render(ctx).indent(indent), 100, 4);

    let diff_row = 1;
    assert_buffer_eq(&term, &["  ✓ Edit (file.rs)", &format!("   1 old code{}1 new code", " ".repeat(41)), "", ""]);

    let theme = &ViewContext::new((100, 4)).theme;
    for col in 0..usize::from(indent) {
        let bg = term.get_style_at(diff_row, col).bg;
        assert_ne!(bg, Some(theme.diff_removed_bg()), "indent col {col} should not inherit diff_removed_bg");
        assert_ne!(bg, Some(theme.diff_added_bg()), "indent col {col} should not inherit diff_added_bg");
    }
}

#[test]
fn sub_agent_tool_call_renders_nested() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call_with_args("c1", "grep", json!({ "pattern": "test" })));

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 2);
    assert!(output[0].contains("explorer"));
    assert!(output[0].starts_with("  "));
    assert!(output[1].starts_with("  └─ "));
    assert!(output[1].contains("grep"));
}

#[test]
fn sub_agent_tool_call_update_appends_chunk() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "grep"));
    progress(&mut statuses, "explorer", sub_agent_tool_update("c1", json!({ "pattern": "updated" })));
    progress(&mut statuses, "explorer", sub_agent_tool_result("c1", "grep"));

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 2);
    assert!(output[1].contains("updated"));
}

#[test]
fn sub_agent_tool_result_shows_checkmark() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "read_file"));
    progress(&mut statuses, "explorer", sub_agent_tool_result("c1", "read_file"));

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 2);
    assert!(output[1].contains("✓"));
}

#[test]
fn sub_agent_tool_result_uses_result_meta() {
    let mut statuses = setup_parent();
    progress(
        &mut statuses,
        "explorer",
        sub_agent_tool_call_with_args("c1", "coding__read_file", json!({ "filePath": "Cargo.toml" })),
    );
    progress(
        &mut statuses,
        "explorer",
        sub_agent_tool_result_with_display_meta("c1", "coding__read_file", "Read file", "Cargo.toml, 156 lines"),
    );

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 2);
    assert!(output[1].contains("✓"));
    assert!(output[1].contains("Read file"));
    assert!(output[1].contains("(Cargo.toml, 156 lines)"));
    assert!(!output[1].contains("filePath"));
}

#[test]
fn sub_agent_tool_error_shows_x() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "read_file"));
    progress(&mut statuses, "explorer", sub_agent_tool_error("c1", "read_file"));

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 2);
    assert!(output[1].contains("✗"));
}

#[test]
fn multiple_sub_agents_render_separate_headers() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "grep"));
    progress(&mut statuses, "writer", sub_agent_tool_call("c2", "write_file"));

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 5);
    assert!(output[0].contains("explorer"));
    assert!(output[3].contains("writer"));
}

#[test]
fn same_name_agents_with_different_task_ids_render_separately() {
    let mut statuses = setup_parent();
    for (task, id, name) in [("task-1", "c1", "grep"), ("task-2", "c2", "read_file"), ("task-3", "c3", "list_files")] {
        statuses.on_sub_agent_progress(&sub_agent_progress_with_task_id(
            "parent-1",
            task,
            "codebase-explorer",
            sub_agent_tool_call(id, name),
        ));
    }

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 8);
    assert!(output[1].contains("grep"));
    assert!(output[4].contains("read_file"));
    assert!(output[7].contains("list_files"));
}

#[test]
fn sub_agent_renders_latest_three_tools_with_overflow() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "grep"));
    progress(&mut statuses, "explorer", sub_agent_tool_result("c1", "grep"));
    progress(&mut statuses, "explorer", sub_agent_tool_call("c2", "read_file"));
    progress(&mut statuses, "explorer", sub_agent_tool_call("c3", "list_files"));
    progress(&mut statuses, "explorer", sub_agent_tool_call("c4", "write_file"));

    let output = render_tool_lines(&statuses, "parent-1");
    assert_eq!(output.len(), 5);
    assert!(output[1].contains("1 earlier tool calls"));
    assert!(output[2].contains("read_file"));
    assert!(output[2].contains("├─"));
    assert!(output[3].contains("list_files"));
    assert!(output[3].contains("├─"));
    assert!(output[4].contains("write_file"));
    assert!(output[4].contains("└─"));
}

#[test]
fn agent_header_shows_spinner_while_running() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "grep"));
    progress(&mut statuses, "explorer", sub_agent_tool_result("c1", "grep"));

    let output = render_tool_lines(&statuses, "parent-1");
    assert!(!output[0].contains('✓'), "Expected spinner, not ✓ in header: {}", output[0]);
}

#[test]
fn agent_header_shows_done_after_done_event() {
    let mut statuses = setup_parent();
    progress(&mut statuses, "explorer", sub_agent_tool_call("c1", "grep"));
    progress(&mut statuses, "explorer", sub_agent_done());

    let output = render_tool_lines(&statuses, "parent-1");
    assert!(output[0].contains('✓'), "Expected ✓ in header: {}", output[0]);
}

#[test]
fn test_display_value_shown_on_completion() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(&ToolCallFactory::default().id("tool-1").name("coding__read_file").build());

    let mut meta_map = serde_json::Map::new();
    meta_map.insert("display_value".into(), "Cargo.toml, 156 lines".into());
    let update = acp::ToolCallUpdate::new(
        "tool-1".to_string(),
        acp::ToolCallUpdateFields::new().title("Read file").status(acp::ToolCallStatus::Completed),
    )
    .meta(meta_map);
    statuses.on_tool_call_update(&update);

    let lines = render_all(&statuses, &["tool-1"], &ctx());
    assert_eq!(lines.len(), 1);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    let text = &output[0];
    assert!(text.contains("Read file"), "Expected display title in output: {text}");
    assert!(text.contains("(Cargo.toml, 156 lines)"), "Expected display value in output: {text}");
}

#[test]
fn test_display_value_shown_while_running() {
    let mut statuses = ToolCallStatuses::new();
    statuses.on_tool_call(
        &ToolCallFactory::default()
            .id("tool-1")
            .name("Read file")
            .raw_input_json(r#"{"file_path":"/path/to/main.rs"}"#)
            .build(),
    );

    let mut meta_map = serde_json::Map::new();
    meta_map.insert("display_value".into(), "main.rs".into());
    let update = acp::ToolCallUpdate::new("tool-1".to_string(), acp::ToolCallUpdateFields::new()).meta(meta_map);
    statuses.on_tool_call_update(&update);

    let lines = render_all(&statuses, &["tool-1"], &ctx());
    assert_eq!(lines.len(), 1);
    let term = render_lines(&lines, 80, 24);
    let output = term.get_lines();
    let text = &output[0];
    assert!(text.contains("(main.rs)"), "Expected display value while running: {text}");
    assert!(!text.contains("file_path"), "Raw args should not appear: {text}");
}
