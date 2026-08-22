use acp_utils::client::AcpEvent;
use acp_utils::notifications::{ContextUsage, ContextUsageParams, McpServerStatus, McpServerStatusEntry};
use agent_client_protocol::schema::v1::{self as acp, SessionId};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::{Duration, Instant};
use wisp::session::workspace_status::WorkspaceStatus;
use wisp::settings::{StatusLineSegmentConfig, StatusLineSettings, StatusLineStyle, UiSettings};

use super::support::*;

fn make_ui(settings: &UiSettings, width: u16, height: u16) -> TestUi {
    TestUiBuilder::new().settings(settings.clone()).dimensions(width, height).build()
}

fn ctrl_c() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
}

fn model_option() -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(
        "model",
        "Model",
        "claude-sonnet",
        vec![acp::SessionConfigSelectOption::new("claude-sonnet", "Claude Sonnet")],
    )
}

fn reasoning_option() -> acp::SessionConfigOption {
    acp::SessionConfigOption::select(
        "reasoning_effort",
        "Reasoning",
        "medium",
        vec![
            acp::SessionConfigSelectOption::new("low", "Low"),
            acp::SessionConfigSelectOption::new("medium", "Medium"),
            acp::SessionConfigSelectOption::new("high", "High"),
        ],
    )
}

fn mode_option() -> acp::SessionConfigOption {
    acp::SessionConfigOption::select("mode", "Mode", "code", vec![acp::SessionConfigSelectOption::new("code", "Code")])
}

fn config_update(options: Vec<acp::SessionConfigOption>) -> AcpEvent {
    use acp_utils::client::AcpEvent as ACP;
    ACP::SessionUpdate {
        session_id: SessionId::new("test-session"),
        update: Box::new(acp::SessionUpdate::ConfigOptionUpdate(acp::ConfigOptionUpdate::new(options))),
    }
}

fn type_and_submit(ui: &mut TestUi, text: &str) {
    ui.type_text(text);
    ui.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn context_usage(used: u32, limit: u32) -> AcpEvent {
    AcpEvent::ContextUsage(ContextUsageParams {
        usage: ContextUsage { input_tokens: used, context_limit: Some(limit), ..ContextUsage::default() },
    })
}

#[test]
fn status_line_renders_all_default_segments() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(100_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("~/code/demo"), "should contain cwd, got:\n{text}");
    assert!(text.contains("main"), "should contain git ref, got:\n{text}");
    assert!(text.contains("aether"), "should contain agent name, got:\n{text}");
    assert!(text.contains("Claude Sonnet"), "should contain model, got:\n{text}");
}

#[test]
fn status_line_shows_reasoning_bar() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option(), reasoning_option()]));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("medium"), "should contain reasoning effort label, got:\n{text}");
}

#[test]
fn status_line_reasoning_hidden_without_option() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();

    let text = ui.viewport_text();
    assert!(
        !text.contains("medium") && !text.contains("low") && !text.contains("high"),
        "reasoning bar should be hidden, got:\n{text}"
    );
}

#[test]
fn status_line_context_gauge_shows_slots_and_tokens() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(100_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("ctx"), "should contain ctx label, got:\n{text}");
    assert!(text.contains("100k"), "should contain used token count, got:\n{text}");
    assert!(text.contains("200k"), "should contain limit token count, got:\n{text}");
}

#[test]
fn status_line_context_gauge_full_has_three_filled_slots() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(200_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("[■■■]"), "full context should show 3 filled slots, got:\n{text}");
}

#[test]
fn status_line_ctrl_c_confirmation_replaces_right_side() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.key(ctrl_c());
    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("Ctrl-C again to exit"), "should show exit warning, got:\n{text}");
    assert!(!text.contains("Claude Sonnet"), "should not show model during confirmation, got:\n{text}");
}

#[test]
fn status_line_model_max_width_truncates() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![StatusLineSegmentConfig::Model { max_width: Some(10) }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 120, 15);
    let options = vec![acp::SessionConfigOption::select(
        "model",
        "Model",
        "very-long-model-name",
        vec![acp::SessionConfigSelectOption::new("very-long-model-name", "Very Long Model Name Indeed")],
    )];
    ui.acp_event(config_update(options));

    ui.draw();

    let text = ui.viewport_text();
    assert!(!text.contains("Very Long Model Name Indeed"), "should truncate by maxWidth, got:\n{text}");
}

#[test]
fn status_line_narrow_wraps_to_two_rows() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![StatusLineSegmentConfig::Agent, StatusLineSegmentConfig::Model { max_width: None }]),
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 40, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();

    let text = ui.viewport_text();

    assert!(text.contains("aether"), "second row should contain agent, got:\n{text}");
    assert!(text.contains("Claude Sonnet"), "second row should contain model, got:\n{text}");
    assert!(text.contains("~/code/demo"), "first row should contain cwd, got:\n{text}");
}

#[test]
fn status_line_wide_stays_on_one_row() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 160, 15);
    ui.acp_event(config_update(vec![model_option(), reasoning_option()]));
    ui.acp_event(context_usage(100_000, 200_000));

    ui.draw();

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
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![
                StatusLineSegmentConfig::Agent,
                StatusLineSegmentConfig::Mode,
                StatusLineSegmentConfig::Model { max_width: None },
            ]),
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 120, 15);

    ui.draw();

    let text = ui.viewport_text();
    assert!(!text.contains("··"), "should not have doubled separators, got:\n{text}");
}

#[test]
fn status_line_empty_right_no_separator_remnant() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings { right: Some(vec![]), ..Default::default() }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 60, 15);

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("~/code/demo"), "should still show left, got:\n{text}");
}

#[test]
fn status_line_text_segment_styled() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            left: Some(vec![StatusLineSegmentConfig::Text {
                value: "custom".to_string(),
                style: Some(StatusLineStyle::Warning),
            }]),
            right: Some(vec![]),
            ..Default::default()
        }),
        content_padding: Some(0),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 80, 15);

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("custom"), "should render text segment, got:\n{text}");
}

#[test]
fn status_line_server_health_hidden_when_healthy() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();

    let text = ui.viewport_text();
    assert!(!text.contains("unhealthy"), "should not show server health when all healthy, got:\n{text}");
    assert!(!text.contains("needs auth"), "should not show server health when all healthy, got:\n{text}");
}

#[test]
fn status_line_server_health_shows_unhealthy() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.acp_event(AcpEvent::McpNotification(acp_utils::notifications::McpNotification::ServerStatus {
        servers: vec![
            McpServerStatusEntry::new("github", McpServerStatus::Connected { tool_count: 5 }),
            McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth),
        ],
    }));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("needs auth") || text.contains("unhealthy"), "should show server health, got:\n{text}");
}

#[test]
fn status_line_server_health_hidden_when_busy() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.acp_event(AcpEvent::McpNotification(acp_utils::notifications::McpNotification::ServerStatus {
        servers: vec![McpServerStatusEntry::new("linear", McpServerStatus::NeedsOAuth)],
    }));

    type_and_submit(&mut ui, "test");

    ui.draw();

    let text = ui.viewport_text();
    assert!(!text.contains("unhealthy"), "should hide server health when agent is busy, got:\n{text}");
    assert!(!text.contains("needs auth"), "should hide server health when agent is busy, got:\n{text}");
}

#[test]
fn status_line_context_color_warning_at_71_percent() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(142_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("ctx"), "should render context gauge, got:\n{text}");
}

#[test]
fn status_line_context_color_error_at_86_percent() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(172_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("ctx"), "should render context gauge, got:\n{text}");
}

#[test]
fn status_line_two_row_never_enters_scrollback() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![StatusLineSegmentConfig::Agent, StatusLineSegmentConfig::Model { max_width: None }]),
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 30, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();

    let sb_text = ui.history_text();
    assert!(!sb_text.contains("aether"), "status line should not be in scrollback:\n{sb_text}");
    assert!(!sb_text.contains("Claude Sonnet"), "status line should not be in scrollback:\n{sb_text}");
}

#[test]
fn status_line_zero_width_no_panic() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 0, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();
}

#[test]
fn status_line_multi_model_comma_separated() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    let options = vec![acp::SessionConfigOption::select(
        "model",
        "Model",
        "claude-sonnet,claude-opus",
        vec![
            acp::SessionConfigSelectOption::new("claude-sonnet", "Claude Sonnet"),
            acp::SessionConfigSelectOption::new("claude-opus", "Claude Opus"),
        ],
    )];
    ui.acp_event(config_update(options));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("Claude Sonnet + Claude Opus"), "should show both models joined with +, got:\n{text}");
}

#[test]
fn status_line_ctrl_c_disarms_after_window() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.key(ctrl_c());
    ui.tick(Instant::now() + Duration::from_secs(2));
    ui.draw();

    let text = ui.viewport_text();
    assert!(!text.contains("Ctrl-C again to exit"), "confirmation should disarm, got:\n{text}");
    assert!(text.contains("Claude Sonnet"), "should show model again, got:\n{text}");
}

#[test]
fn status_line_reordered_segments_render_in_configured_order() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            left: Some(vec![StatusLineSegmentConfig::Agent]),
            right: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 120, 15);

    ui.draw();

    let text = ui.viewport_text();
    let agent_pos = text.find("aether").expect("should contain agent");
    let cwd_pos = text.find("~/code/demo").expect("should contain cwd");
    assert!(agent_pos < cwd_pos, "agent should render before cwd, got:\n{text}");
}

#[test]
fn status_line_unicode_truncation_uses_display_width() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            right: Some(vec![StatusLineSegmentConfig::Cwd { max_width: Some(12) }]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let workspace_status = WorkspaceStatus::new("~/café/longpath/here", Some("main".to_string()));
    let mut ui = TestUiBuilder::new().settings(settings).workspace_status(workspace_status).dimensions(120, 15).build();

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains('…'), "should truncate with ellipsis at maxWidth, got:\n{text}");
}

#[test]
fn status_line_mode_displays_current_mode() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option(), mode_option()]));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("Code"), "should show current mode, got:\n{text}");
}

#[test]
fn status_line_reasoning_max_effort_uses_warning_color() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    let options = vec![acp::SessionConfigOption::select(
        "reasoning_effort",
        "Reasoning",
        "max",
        vec![
            acp::SessionConfigSelectOption::new("low", "Low"),
            acp::SessionConfigSelectOption::new("medium", "Medium"),
            acp::SessionConfigSelectOption::new("high", "High"),
            acp::SessionConfigSelectOption::new("max", "Max"),
        ],
    )];
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(config_update(options));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("max"), "should show max reasoning, got:\n{text}");
}

#[test]
fn status_line_hidden_segments_do_not_appear() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![StatusLineSegmentConfig::Agent]),
            ..Default::default()
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();

    let text = ui.viewport_text();
    assert!(!text.contains("Claude Sonnet"), "model should be hidden, got:\n{text}");
    assert!(!text.contains("main"), "git ref should be hidden, got:\n{text}");
}

#[test]
fn status_line_narrow_without_right_produces_one_line() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![
                StatusLineSegmentConfig::Cwd { max_width: None },
                StatusLineSegmentConfig::Agent,
                StatusLineSegmentConfig::Model { max_width: None },
            ]),
            right: Some(vec![]),
        }),
        ..Default::default()
    };
    let mut ui = make_ui(&settings, 30, 15);
    ui.acp_event(config_update(vec![model_option()]));

    ui.draw();

    let text = ui.viewport_text();
    let status_rows: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("~/code/demo") || line.contains("aether") || line.contains("Claude Sonnet"))
        .collect();
    assert_eq!(status_rows.len(), 1, "omitting right should produce exactly 1 status row, got:\n{text}");
}

#[test]
fn status_line_two_row_overflow_truncates_with_ellipsis() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![StatusLineSegmentConfig::Agent]),
        }),
        ..Default::default()
    };
    let workspace_status = WorkspaceStatus::new(
        "~/very-long-directory-name-that-should-be-truncated",
        Some("feature/very-long-branch-name".to_string()),
    );
    let mut ui = TestUiBuilder::new().settings(settings).workspace_status(workspace_status).dimensions(20, 15).build();

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains('…'), "narrow two-row overflow should truncate with ellipsis, got:\n{text}");
}

#[test]
fn status_line_two_row_unicode_overflow_truncates_at_display_width() {
    let settings = UiSettings {
        status_line: Some(StatusLineSettings {
            separator: Some(" · ".to_string()),
            left: Some(vec![StatusLineSegmentConfig::Cwd { max_width: None }]),
            right: Some(vec![StatusLineSegmentConfig::Agent]),
        }),
        ..Default::default()
    };
    let workspace_status = WorkspaceStatus::new("~/日本語の長いディレクトリパス", Some("main".to_string()));
    let mut ui = TestUiBuilder::new().settings(settings).workspace_status(workspace_status).dimensions(20, 15).build();

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains('…'), "unicode overflow should truncate at display width with ellipsis, got:\n{text}");
}

#[test]
fn context_zero_limit_renders_empty_gauge() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(0, 0));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("ctx"), "should render context gauge even with zero limit, got:\n{text}");
    assert!(text.contains("[···]"), "zero usage should show empty slots, got:\n{text}");
}

#[test]
fn context_exact_boundary_71_percent_shows_warning() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(142_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("ctx"), "context gauge should render at 71% boundary, got:\n{text}");
}

#[test]
fn context_exact_boundary_86_percent_shows_error() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(172_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("ctx"), "context gauge should render at 86% boundary, got:\n{text}");
}

#[test]
fn reasoning_none_effort_shows_empty_bar() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    let options = vec![
        model_option(),
        acp::SessionConfigOption::select(
            "reasoning_effort",
            "Reasoning",
            "none",
            vec![
                acp::SessionConfigSelectOption::new("none", "None"),
                acp::SessionConfigSelectOption::new("low", "Low"),
                acp::SessionConfigSelectOption::new("medium", "Medium"),
            ],
        ),
    ];
    ui.acp_event(config_update(options));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("none"), "should show none reasoning effort, got:\n{text}");
    assert!(text.contains("[··]"), "should show empty slot bar with 2 levels, got:\n{text}");
}

#[test]
fn context_small_tokens_renders_correctly() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(500, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("500"), "should show small token count unformatted, got:\n{text}");
    assert!(text.contains("ctx"), "should render context, got:\n{text}");
}

#[test]
fn context_exactly_half_fills_one_and_a_half_slots() {
    let settings = UiSettings::default();
    let mut ui = make_ui(&settings, 120, 15);
    ui.acp_event(config_update(vec![model_option()]));
    ui.acp_event(context_usage(100_000, 200_000));

    ui.draw();

    let text = ui.viewport_text();
    assert!(text.contains("100k"), "should show formatted count, got:\n{text}");
    assert!(text.contains("200k"), "should show limit, got:\n{text}");
}
