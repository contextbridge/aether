use clankerdiff_ratatui::composite_color;
use clankerdiff_ratatui::theme::{ReviewTheme, Rgba, ThemeError};
use client_core::workspace_status::WorkspaceStatus;
use wisp::settings::{StatusLineSegmentConfig as Segment, StatusLineSettings, StatusLineStyle};

use super::support::*;

const CWD: Segment = Segment::Cwd { max_width: None };
const MODEL: Segment = Segment::Model { max_width: None };
const LEVELS: &[&str] = &["low", "medium", "high"];

type Role = fn(&Theme) -> Color;

#[test]
fn every_builtin_renders_distinct_status_roles() -> Result<(), ThemeError> {
    for descriptor in ReviewTheme::catalog() {
        let review = ReviewTheme::builtin(&descriptor.id)?;
        let palette = review.ui;
        let settings =
            UiSettings { theme: wisp::settings::ThemeSettings::Builtin { id: descriptor.id }, ..Default::default() };
        let mut ui = TestUiBuilder::new().settings(settings).dimensions(180, 20).build();
        ui.acp_event(config_update(vec![
            sonnet_model(),
            reasoning_option("medium", LEVELS),
            mode_option("code", &["code"]),
        ]));
        ui.acp_event(context_usage(150_000, 200_000));
        ui.acp_event(mcp_notification(vec![McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)]));
        let buffer = ui.viewport();
        for (text, color) in [
            ("aether", palette.info),
            ("medium", palette.info),
            ("main", palette.positive),
            ("Claude Sonnet", palette.positive),
            ("~/code/demo", palette.text_secondary),
            ("ctx", palette.warning),
            ("1 server needs auth", palette.warning),
        ] {
            assert_text_color(&buffer, text, composite_color(color, palette.canvas));
        }
        ui.acp_event(context_usage(180_000, 200_000));
        assert_text_color(&ui.viewport(), "ctx", composite_color(palette.destructive, palette.canvas));
    }
    Ok(())
}

#[test]
fn ui_only_theme_switch_recolors_status_mentions_and_message_surfaces() {
    let mut ui = TestUi::with_dimensions(180, 20);
    ui.submit("user message");
    ui.complete_prompt(acp::StopReason::EndTurn);
    ui.type_text("@src/lib.rs ");
    let mut review = ReviewTheme::default();
    let syntax_revision = review.syntax.revision();
    for (info, surface) in [
        (Rgba::new(40, 160, 220, 255), Rgba::new(35, 45, 55, 255)),
        (Rgba::new(90, 180, 240, 255), Rgba::new(45, 55, 65, 255)),
    ] {
        review.ui.info = info;
        review.ui.surface = surface;
        let settings = Box::new(ui.app().ui_settings().clone());
        ui.deliver_result(CommandResult::Host(HostCommandResult::ThemeApplied(Ok((
            settings,
            Theme::from_review(review.clone()),
        )))));
        let buffer = ui.viewport();
        let color = composite_color(info, review.ui.canvas);
        assert_text_color(&buffer, "aether", color);
        assert_text_color(&buffer, "@src/lib.rs", color);
        assert!(
            buffer
                .content()
                .iter()
                .any(|cell| cell.symbol() == "u" && cell.bg == composite_color(surface, review.ui.canvas))
        );
    }
    assert_eq!(ui.app().theme().review().syntax.revision(), syntax_revision);
}

#[test]
fn status_line_renders_all_default_segments() {
    let mut ui = status_ui();
    ui.acp_event(context_usage(100_000, 200_000));

    for segment in ["~/code/demo", "main", "aether", "Claude Sonnet"] {
        ui.assert_viewport_contains(segment);
    }
}

#[test]
fn status_line_reasoning_hidden_without_option() {
    let mut ui = status_ui();

    for level in LEVELS {
        ui.assert_viewport_not_contains(level);
    }
}

#[test]
fn reasoning_bar_fills_with_the_current_effort() {
    let cases: [(&str, &[&str], &str, Role); 3] = [
        ("medium", LEVELS, "medium [■■·]", |theme| theme.info),
        ("default", &["default", "low", "medium"], "default [··]", |theme| theme.text_secondary),
        ("max", &["low", "medium", "high", "max"], "max [■■■■]", |theme| theme.warning),
    ];
    for (current, levels, bar, color) in cases {
        let mut ui = status_ui();
        ui.acp_event(config_update(vec![sonnet_model(), reasoning_option(current, levels)]));

        assert_text_color(&ui.viewport(), bar, color(ui.app().theme()));
    }
}

#[test]
fn context_gauge_fills_slots_and_warns_as_the_window_fills() {
    let cases: [(u64, u64, &str, Role); 6] = [
        (0, 0, "ctx [···] 0 / 0", |theme| theme.text_secondary),
        (500, 200_000, "ctx [···] 500 / 200k", |theme| theme.text_secondary),
        (100_000, 200_000, "ctx [■■·] 100k / 200k", |theme| theme.text_secondary),
        (142_000, 200_000, "ctx [■■·] 142k / 200k", |theme| theme.warning),
        (172_000, 200_000, "ctx [■■■] 172k / 200k", |theme| theme.error),
        (200_000, 200_000, "ctx [■■■] 200k / 200k", |theme| theme.error),
    ];
    for (used, limit, gauge, color) in cases {
        let mut ui = status_ui();
        ui.acp_event(context_usage(used, limit));

        assert_text_color(&ui.viewport(), gauge, color(ui.app().theme()));
    }
}

#[test]
fn status_line_ctrl_c_confirmation_replaces_right_side_until_it_disarms() {
    let mut ui = status_ui();

    ui.key(ctrl('c'));
    ui.assert_viewport_contains("Ctrl-C again to exit");
    ui.assert_viewport_not_contains("Claude Sonnet");

    ui.tick(Instant::now() + Duration::from_secs(2));
    ui.assert_viewport_not_contains("Ctrl-C again to exit");
    ui.assert_viewport_contains("Claude Sonnet");
}

#[test]
fn status_line_model_max_width_truncates() {
    let settings = segments(vec![CWD], vec![Segment::Model { max_width: Some(10) }]);
    let mut ui = TestUiBuilder::new().settings(settings).dimensions(120, 15).build();
    ui.acp_event(config_update(vec![select("model", "Model", "long", &[("long", "Very Long Model Name Indeed")])]));

    ui.assert_viewport_not_contains("Very Long Model Name Indeed");
}

#[test]
fn status_line_narrow_wraps_to_two_rows() {
    let settings = segments(vec![CWD], vec![Segment::Agent, MODEL]);
    let mut ui =
        TestUiBuilder::new().settings(settings).dimensions(40, 15).config_options(vec![sonnet_model()]).build();

    ui.assert_viewport_contains("aether");
    ui.assert_viewport_contains("Claude Sonnet");
    ui.assert_viewport_contains("~/code/demo");
}

#[test]
fn status_line_wide_stays_on_one_row() {
    let mut ui = TestUiBuilder::new()
        .dimensions(160, 15)
        .config_options(vec![sonnet_model(), reasoning_option("medium", LEVELS)])
        .build();
    ui.acp_event(context_usage(100_000, 200_000));

    let text = ui.viewport_text();
    assert!(text.contains("~/code/demo"), "status line should be on viewport, got:\n{text}");

    let status_rows: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("~/code/demo") || line.contains("aether") || line.contains("ctx"))
        .collect();
    assert_eq!(status_rows.len(), 1, "all status segments should be on one row, got:\n{text}");
}

#[test]
fn status_line_missing_segments_no_doubled_separators() {
    let settings = segments(vec![CWD], vec![Segment::Agent, Segment::Mode, MODEL]);
    let mut ui = TestUiBuilder::new().settings(settings).dimensions(120, 15).build();

    ui.assert_viewport_not_contains("··");
}

#[test]
fn status_line_empty_right_no_separator_remnant() {
    let settings = segments(vec![CWD, Segment::GitRef], vec![]);
    let mut ui = TestUiBuilder::new().settings(settings).dimensions(60, 15).build();

    ui.assert_viewport_contains("~/code/demo · main");
    ui.assert_viewport_not_contains("main ·");
}

#[test]
fn status_line_text_segment_styled() {
    let text = Segment::Text { value: "custom".to_string(), style: Some(StatusLineStyle::Warning) };
    let settings = UiSettings { content_padding: Some(0), ..segments(vec![text], vec![]) };
    let mut ui = TestUiBuilder::new().settings(settings).dimensions(80, 15).build();

    assert_text_color(&ui.viewport(), "custom", ui.app().theme().warning);
}

#[test]
fn status_line_server_health_counts_unhealthy_servers_while_idle() {
    let connected = || McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 });
    let needs_auth = || McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth);
    let failed = || McpServerStatusEntry::new("slack", McpServerStatus::Failed { error: "timeout".to_string() });
    let cases = [
        (vec![connected()], false, None),
        (vec![connected(), needs_auth()], false, Some("1 server needs auth")),
        (vec![needs_auth(), failed()], false, Some("2 servers unhealthy")),
        (vec![needs_auth()], true, None),
    ];
    for (servers, busy, shown) in cases {
        let mut ui = status_ui();
        ui.acp_event(mcp_notification(servers));
        if busy {
            ui.submit("test");
        }

        if let Some(health) = shown {
            ui.assert_viewport_contains(health);
        } else {
            ui.assert_viewport_not_contains("unhealthy");
            ui.assert_viewport_not_contains("needs auth");
        }
    }
}

#[test]
fn status_line_two_row_never_enters_scrollback() {
    let settings = segments(vec![CWD], vec![Segment::Agent, MODEL]);
    let mut ui =
        TestUiBuilder::new().settings(settings).dimensions(30, 15).config_options(vec![sonnet_model()]).build();

    ui.assert_history_not_contains("aether");
    ui.assert_history_not_contains("Claude Sonnet");
}

#[test]
fn status_line_zero_width_no_panic() {
    let mut ui = TestUiBuilder::new().dimensions(0, 15).config_options(vec![sonnet_model()]).build();

    ui.draw();
}

#[test]
fn status_line_multi_model_comma_separated() {
    let mut ui = status_ui();
    let both = &[("claude-sonnet", "Claude Sonnet"), ("claude-opus", "Claude Opus")];
    ui.acp_event(config_update(vec![select("model", "Model", "claude-sonnet,claude-opus", both)]));

    ui.assert_viewport_contains("Claude Sonnet + Claude Opus");
}

#[test]
fn status_line_reordered_segments_render_in_configured_order() {
    let mut ui = TestUiBuilder::new().settings(segments(vec![Segment::Agent], vec![CWD])).dimensions(120, 15).build();

    let text = ui.viewport_text();
    let agent_pos = text.find("aether").expect("should contain agent");
    let cwd_pos = text.find("~/code/demo").expect("should contain cwd");
    assert!(agent_pos < cwd_pos, "agent should render before cwd, got:\n{text}");
}

#[test]
fn status_line_unicode_truncation_uses_display_width() {
    let settings = segments(vec![Segment::GitRef], vec![Segment::Cwd { max_width: Some(12) }]);
    let workspace_status =
        WorkspaceStatus { display_dir: "~/café/longpath/here".to_string(), git_ref: Some("main".to_string()) };
    let mut ui = TestUiBuilder::new().settings(settings).workspace_status(workspace_status).dimensions(120, 15).build();

    ui.assert_viewport_contains("…");
}

#[test]
fn status_line_mode_displays_current_mode() {
    let mut ui = status_ui();
    ui.acp_event(config_update(vec![sonnet_model(), select("mode", "Mode", "code", &[("code", "Code")])]));

    ui.assert_viewport_contains("Code");
}

#[test]
fn status_line_hidden_segments_do_not_appear() {
    let settings = segments(vec![CWD], vec![Segment::Agent]);
    let mut ui =
        TestUiBuilder::new().settings(settings).dimensions(120, 15).config_options(vec![sonnet_model()]).build();

    ui.assert_viewport_not_contains("Claude Sonnet");
    ui.assert_viewport_not_contains("main");
}

#[test]
fn status_line_narrow_without_right_produces_one_line() {
    let settings = segments(vec![CWD, Segment::Agent, MODEL], vec![]);
    let mut ui =
        TestUiBuilder::new().settings(settings).dimensions(30, 15).config_options(vec![sonnet_model()]).build();

    let text = ui.viewport_text();
    let status_rows: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("~/code/demo") || line.contains("aether") || line.contains("Claude Sonnet"))
        .collect();
    assert_eq!(status_rows.len(), 1, "omitting right should produce exactly 1 status row, got:\n{text}");
}

#[test]
fn status_line_two_row_overflow_truncates_at_display_width() {
    for display_dir in ["~/very-long-directory-name-that-should-be-truncated", "~/日本語の長いディレクトリパス"]
    {
        let settings = segments(vec![CWD], vec![Segment::Agent]);
        let workspace_status =
            WorkspaceStatus { display_dir: display_dir.to_string(), git_ref: Some("main".to_string()) };
        let mut ui =
            TestUiBuilder::new().settings(settings).workspace_status(workspace_status).dimensions(20, 15).build();

        ui.assert_viewport_contains("…");
    }
}

fn segments(left: Vec<Segment>, right: Vec<Segment>) -> UiSettings {
    let status_line = StatusLineSettings { left: Some(left), right: Some(right), ..Default::default() };
    UiSettings { status_line: Some(status_line), ..Default::default() }
}

fn assert_text_color(buffer: &Buffer, text: &str, color: Color) {
    let symbols: Vec<_> = text.chars().map(|character| character.to_string()).collect();
    let matched = (buffer.area.top()..buffer.area.bottom()).any(|y| {
        (buffer.area.left()..buffer.area.right()).any(|x| {
            symbols.iter().enumerate().all(|(offset, symbol)| {
                let Ok(offset) = u16::try_from(offset) else {
                    return false;
                };
                buffer
                    .cell((x.saturating_add(offset), y))
                    .is_some_and(|cell| cell.symbol() == symbol && cell.fg == color)
            })
        })
    });
    assert!(matched, "{text:?} should be {color:?}:\n{}", buffer_text(buffer));
}
