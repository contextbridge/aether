use crate::components::context_bar::{context_bar, context_color};
use crate::components::reasoning_bar::{reasoning_bar, reasoning_color};
use crate::settings::{ResolvedStatusLineSettings, StatusLineSegmentConfig, StatusLineStyle};
use crate::workspace::WorkspaceStatus;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{
    self as acp, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions,
};
use tui::{Color, FitOptions, Frame, Line, ViewContext, display_width_text};
use utils::ReasoningEffort;

pub use crate::components::context_bar::ContextUsageDisplay;

#[doc = include_str!("../docs/status_line.md")]
pub struct StatusLine<'a> {
    pub workspace_status: &'a WorkspaceStatus,
    pub agent_name: &'a str,
    pub config_options: &'a [SessionConfigOption],
    pub context_usage: Option<ContextUsageDisplay>,
    pub waiting_for_response: bool,
    pub unhealthy_server_count: usize,
    pub content_padding: usize,
    pub exit_confirmation_active: bool,
    pub settings: &'a ResolvedStatusLineSettings,
}

impl StatusLine<'_> {
    pub fn render(&self, context: &ViewContext) -> Frame {
        let width = context.size.width as usize;
        if width == 0 {
            return Frame::new(vec![Line::default()]);
        }

        let left = self.render_left_section(context);
        let left_len = left.display_width();
        let right = self.render_right_section(context);
        let right_len = right.display_width();

        let lines = if right.is_empty() {
            vec![truncate_to_width(left, width)]
        } else if left_len + 1 + right_len <= width {
            vec![single_status_line(left, &right, width)]
        } else {
            vec![truncate_to_width(left, width), align_left(&right, self.content_padding, width)]
        };

        Frame::new(lines).fit(context.size.width, FitOptions::truncate())
    }

    fn render_left_section(&self, context: &ViewContext) -> Line {
        let mut line = Line::default();
        line.push_text(" ".repeat(self.content_padding));
        line.append_line(&self.join_segments(&self.settings.left, context));
        line
    }

    fn render_right_section(&self, context: &ViewContext) -> Line {
        if self.exit_confirmation_active {
            let mut line = Line::default();
            line.push_styled("Ctrl-C again to exit", context.theme.warning());
            return line;
        }
        self.join_segments(&self.settings.right, context)
    }

    fn join_segments(&self, segments: &[StatusLineSegmentConfig], context: &ViewContext) -> Line {
        let mut line = Line::default();
        let mut first = true;
        for segment in segments {
            let Some(segment_line) = render_segment(segment, self, context) else { continue };
            if segment_line.is_empty() {
                continue;
            }
            if !first {
                line.push_styled(&self.settings.separator, context.theme.text_secondary());
            }
            line.append_line(&segment_line);
            first = false;
        }
        line
    }
}

fn render_segment(segment: &StatusLineSegmentConfig, status: &StatusLine<'_>, context: &ViewContext) -> Option<Line> {
    match segment {
        StatusLineSegmentConfig::Cwd { max_width } => {
            let mut line = Line::default();
            let dir = truncate_text(&status.workspace_status.display_dir, *max_width);
            line.push_styled(&dir, context.theme.secondary());
            Some(line)
        }
        StatusLineSegmentConfig::GitRef => {
            let git_ref = status.workspace_status.git_ref.as_deref()?;
            let mut line = Line::default();
            line.push_styled(git_ref, context.theme.success());
            Some(line)
        }
        StatusLineSegmentConfig::Agent => {
            let mut line = Line::default();
            line.push_styled(status.agent_name, context.theme.info());
            Some(line)
        }
        StatusLineSegmentConfig::Mode => {
            let mode_text = extract_mode_display(status.config_options)?;
            let mut line = Line::default();
            line.push_styled(&mode_text, context.theme.secondary());
            Some(line)
        }
        StatusLineSegmentConfig::Model { max_width } => {
            let model_summary = extract_model_display(status.config_options)?;
            let truncated = truncate_text(&model_summary, *max_width);
            let mut line = Line::default();
            line.push_styled(&truncated, context.theme.success());
            Some(line)
        }
        StatusLineSegmentConfig::Reasoning => {
            let reasoning_levels = extract_reasoning_levels(status.config_options);
            if reasoning_levels.is_empty() {
                return None;
            }
            let reasoning_effort = extract_reasoning_effort(status.config_options);
            let mut line = Line::default();
            line.push_styled(
                reasoning_bar(reasoning_effort, reasoning_levels.len()),
                reasoning_color(reasoning_effort, reasoning_levels.len(), &context.theme),
            );
            Some(line)
        }
        StatusLineSegmentConfig::Context => {
            let usage = status.context_usage?;
            let mut line = Line::default();
            line.push_styled(context_bar(usage), context_color(usage, &context.theme));
            Some(line)
        }
        StatusLineSegmentConfig::ServerHealth => {
            if status.waiting_for_response || status.unhealthy_server_count == 0 {
                return None;
            }
            let count = status.unhealthy_server_count;
            let msg = if count == 1 { "1 server needs auth".to_string() } else { format!("{count} servers unhealthy") };
            let mut line = Line::default();
            line.push_styled(&msg, context.theme.warning());
            Some(line)
        }
        StatusLineSegmentConfig::Text { value, style } => {
            let color = style.map_or_else(|| context.theme.secondary(), |s| semantic_color(s, context));
            let mut line = Line::default();
            line.push_styled(value, color);
            Some(line)
        }
    }
}

fn semantic_color(style: StatusLineStyle, context: &ViewContext) -> Color {
    match style {
        StatusLineStyle::Primary => context.theme.text_primary(),
        StatusLineStyle::Secondary => context.theme.secondary(),
        StatusLineStyle::Muted => context.theme.text_secondary(),
        StatusLineStyle::Info => context.theme.info(),
        StatusLineStyle::Success => context.theme.success(),
        StatusLineStyle::Warning => context.theme.warning(),
        StatusLineStyle::Error => context.theme.error(),
    }
}

fn single_status_line(mut left: Line, right: &Line, width: usize) -> Line {
    let left_len = left.display_width();
    let right_len = right.display_width();
    let padding = width.saturating_sub(left_len + right_len);
    left.push_text(" ".repeat(padding));
    left.append_line(right);
    left
}

fn align_left(right: &Line, content_padding: usize, width: usize) -> Line {
    let mut line = Line::default();
    line.push_text(" ".repeat(content_padding));
    line.append_line(right);
    truncate_to_width(line, width)
}

fn truncate_to_width(line: Line, width: usize) -> Line {
    let current = line.display_width();
    if current <= width {
        return line;
    }
    Frame::new(vec![line])
        .fit(u16::try_from(width).unwrap_or(u16::MAX), FitOptions::truncate())
        .into_lines()
        .into_iter()
        .next()
        .unwrap_or_default()
}

fn truncate_text(text: &str, max_width: Option<u16>) -> String {
    let Some(max_width) = max_width.map(usize::from) else {
        return text.to_string();
    };
    if display_width_text(text) <= max_width {
        return text.to_string();
    }
    let mut result = String::new();
    let mut current_width = 0;
    for ch in text.chars() {
        let char_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
        if current_width + char_width > max_width.saturating_sub(1) {
            result.push('…');
            break;
        }
        result.push(ch);
        current_width += char_width;
    }
    result
}

/// Extract the parsed reasoning levels from config options (excludes "none").
pub(crate) fn extract_reasoning_levels(config_options: &[SessionConfigOption]) -> Vec<ReasoningEffort> {
    let Some(option) = config_options.iter().find(|o| o.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str())
    else {
        return Vec::new();
    };
    let SessionConfigKind::Select(ref select) = option.kind else {
        return Vec::new();
    };
    let SessionConfigSelectOptions::Ungrouped(ref options) = select.options else {
        return Vec::new();
    };
    options.iter().filter_map(|o| o.value.0.as_ref().parse().ok()).collect()
}

pub(crate) fn is_cycleable_mode_option(option: &SessionConfigOption) -> bool {
    matches!(option.kind, SessionConfigKind::Select(_)) && option.category == Some(SessionConfigOptionCategory::Mode)
}

pub(crate) fn option_display_name(
    options: &SessionConfigSelectOptions,
    current_value: &acp::SessionConfigValueId,
) -> Option<String> {
    match options {
        SessionConfigSelectOptions::Ungrouped(options) => {
            options.iter().find(|option| &option.value == current_value).map(|option| option.name.clone())
        }
        SessionConfigSelectOptions::Grouped(groups) => groups
            .iter()
            .flat_map(|group| group.options.iter())
            .find(|option| &option.value == current_value)
            .map(|option| option.name.clone()),
        _ => None,
    }
}

pub(crate) fn extract_select_display(config_options: &[SessionConfigOption], id: ConfigOptionId) -> Option<String> {
    let option = config_options.iter().find(|option| option.id.0.as_ref() == id.as_str())?;

    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };

    option_display_name(&select.options, &select.current_value)
}

pub(crate) fn extract_mode_display(config_options: &[SessionConfigOption]) -> Option<String> {
    extract_select_display(config_options, ConfigOptionId::Mode)
}

pub(crate) fn extract_model_display(config_options: &[SessionConfigOption]) -> Option<String> {
    let option = config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::Model.as_str())?;

    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };

    let options = match &select.options {
        SessionConfigSelectOptions::Ungrouped(options) => options,
        SessionConfigSelectOptions::Grouped(_) => {
            return extract_select_display(config_options, ConfigOptionId::Model);
        }
        _ => return None,
    };

    let current = select.current_value.0.as_ref();
    if current.contains(',') {
        let names: Vec<&str> = current
            .split(',')
            .filter_map(|part| {
                let trimmed = part.trim();
                options.iter().find(|option| option.value.0.as_ref() == trimmed).map(|option| option.name.as_str())
            })
            .collect();
        if names.is_empty() { None } else { Some(names.join(" + ")) }
    } else {
        extract_select_display(config_options, ConfigOptionId::Model)
    }
}

pub(crate) fn extract_reasoning_effort(config_options: &[SessionConfigOption]) -> Option<ReasoningEffort> {
    let option =
        config_options.iter().find(|option| option.id.0.as_ref() == ConfigOptionId::ReasoningEffort.as_str())?;

    let SessionConfigKind::Select(ref select) = option.kind else {
        return None;
    };

    ReasoningEffort::parse(&select.current_value.0).unwrap_or(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_CONTENT_PADDING;
    use crate::settings::StatusLineSettings;
    use crate::workspace::WorkspaceStatus;

    fn default_settings() -> ResolvedStatusLineSettings {
        StatusLineSettings::resolved_defaults()
    }

    fn test_workspace_status() -> WorkspaceStatus {
        WorkspaceStatus::new("~/code/aether-2", Some("main".to_string()))
    }

    fn status_line<'a>(
        workspace_status: &'a WorkspaceStatus,
        settings: &'a ResolvedStatusLineSettings,
    ) -> StatusLine<'a> {
        StatusLine {
            workspace_status,
            agent_name: "test-agent",
            config_options: &[],
            context_usage: None,
            waiting_for_response: false,
            unhealthy_server_count: 0,
            content_padding: DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: false,
            settings,
        }
    }

    fn model_option() -> SessionConfigOption {
        acp::SessionConfigOption::select(
            "model",
            "Model",
            "claude-sonnet",
            vec![acp::SessionConfigSelectOption::new("claude-sonnet", "Claude Sonnet")],
        )
    }

    fn reasoning_option() -> SessionConfigOption {
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

    #[test]
    fn reasoning_bar_hidden_without_reasoning_option() {
        let options = vec![model_option()];
        let workspace_status = test_workspace_status();
        let settings = default_settings();
        let status = StatusLine { config_options: &options, ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(
            !text.contains("reasoning"),
            "reasoning bar should be hidden when no reasoning_effort option exists, got: {text}"
        );
    }

    #[test]
    fn reasoning_bar_shown_with_reasoning_option() {
        let options = vec![model_option(), reasoning_option()];
        let workspace_status = test_workspace_status();
        let settings = default_settings();
        let status = StatusLine { config_options: &options, ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("medium"), "reasoning bar should use current reasoning effort as its label, got: {text}");
        assert!(!text.contains("reasoning"), "reasoning bar should not use a generic reasoning label, got: {text}");
    }

    #[test]
    fn wraps_right_side_onto_second_line_when_too_narrow() {
        let options = vec![model_option(), reasoning_option()];
        let workspace_status = test_workspace_status();
        let settings = default_settings();
        let status = StatusLine { config_options: &options, ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((60, 40));
        let frame = status.render(&context);
        let left = frame.lines()[0].plain_text();
        let right = frame.lines()[1].plain_text();

        assert_eq!(frame.lines().len(), 2);
        assert!(left.contains("aether-2"));
        assert!(right.contains("test-agent"));
        assert!(right.contains("Claude Sonnet"));
        assert_eq!(right.find("test-agent"), Some(DEFAULT_CONTENT_PADDING));
    }

    #[test]
    fn stays_on_one_line_when_it_fits() {
        let options = vec![model_option(), reasoning_option()];
        let workspace_status = test_workspace_status();
        let settings = default_settings();
        let status = StatusLine { config_options: &options, ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        assert_eq!(frame.lines().len(), 1, "wide status line should stay on a single row");
    }

    #[test]
    fn extract_reasoning_levels_empty_without_option() {
        let options = vec![model_option()];
        assert!(extract_reasoning_levels(&options).is_empty());
    }

    #[test]
    fn extract_reasoning_levels_nonempty_with_option() {
        let options = vec![model_option(), reasoning_option()];
        assert!(!extract_reasoning_levels(&options).is_empty());
    }

    #[test]
    fn default_status_line_contains_all_segments() {
        let options = vec![model_option(), reasoning_option()];
        let workspace_status = test_workspace_status();
        let settings = default_settings();
        let status = StatusLine {
            config_options: &options,
            context_usage: Some(ContextUsageDisplay::new(144_000, 200_000)),
            ..status_line(&workspace_status, &settings)
        };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("aether-2"));
        assert!(text.contains("main"));
        assert!(text.contains("test-agent"));
        assert!(text.contains("Claude Sonnet"));
        assert!(text.contains("medium"));
    }

    #[test]
    fn reordered_segments_render_in_configured_order() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![StatusLineSegmentConfig::Agent],
            right: vec![StatusLineSegmentConfig::Cwd { max_width: None }],
        };
        let workspace_status = test_workspace_status();
        let status = StatusLine { agent_name: "my-agent", ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        let agent_pos = text.find("my-agent").expect("should contain agent");
        let cwd_pos = text.find("aether-2").expect("should contain cwd");
        assert!(agent_pos < cwd_pos);
    }

    #[test]
    fn hidden_segments_do_not_appear() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![StatusLineSegmentConfig::Cwd { max_width: None }],
            right: vec![StatusLineSegmentConfig::Agent],
        };
        let workspace_status = test_workspace_status();
        let status = StatusLine {
            agent_name: "my-agent",
            config_options: &[model_option()],
            ..status_line(&workspace_status, &settings)
        };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(!text.contains("Claude Sonnet"));
        assert!(!text.contains("main"));
    }

    #[test]
    fn missing_segments_no_doubled_separators() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![StatusLineSegmentConfig::Cwd { max_width: None }],
            right: vec![
                StatusLineSegmentConfig::Agent,
                StatusLineSegmentConfig::Mode,
                StatusLineSegmentConfig::Model { max_width: None },
            ],
        };
        let workspace_status = WorkspaceStatus::new("~/code/foo", None);
        let status = StatusLine { config_options: &[model_option()], ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(!text.contains("··"));
    }

    #[test]
    fn model_max_width_truncates_long_names() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![StatusLineSegmentConfig::Cwd { max_width: None }],
            right: vec![StatusLineSegmentConfig::Model { max_width: Some(10) }],
        };
        let workspace_status = test_workspace_status();
        let options = vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            "very-long-model-name",
            vec![acp::SessionConfigSelectOption::new("very-long-model-name", "Very Long Model Name Indeed")],
        )];
        let status =
            StatusLine { agent_name: "test", config_options: &options, ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(!text.contains("Very Long Model Name Indeed"));
    }

    #[test]
    fn narrow_width_with_right_section_produces_two_lines() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![StatusLineSegmentConfig::Cwd { max_width: None }],
            right: vec![StatusLineSegmentConfig::Agent, StatusLineSegmentConfig::Model { max_width: None }],
        };
        let workspace_status = test_workspace_status();
        let options = vec![model_option()];
        let status = StatusLine {
            agent_name: "test-agent-with-a-long-name",
            config_options: &options,
            ..status_line(&workspace_status, &settings)
        };

        let context = ViewContext::new((30, 40));
        let frame = status.render(&context);
        assert!(
            frame.lines().len() == 2,
            "right section should produce exactly 2 lines when content doesn't fit, got {} lines",
            frame.lines().len()
        );
    }

    #[test]
    fn narrow_width_without_right_section_produces_one_line() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![
                StatusLineSegmentConfig::Cwd { max_width: None },
                StatusLineSegmentConfig::Agent,
                StatusLineSegmentConfig::Model { max_width: None },
            ],
            right: vec![],
        };
        let workspace_status = test_workspace_status();
        let options = vec![model_option()];
        let status = StatusLine {
            agent_name: "test-agent-with-a-long-name",
            config_options: &options,
            ..status_line(&workspace_status, &settings)
        };

        let context = ViewContext::new((30, 40));
        let frame = status.render(&context);
        assert_eq!(frame.lines().len(), 1, "omitting right should produce exactly 1 line");
    }

    #[test]
    fn exit_confirmation_replaces_right_side() {
        let settings = default_settings();
        let workspace_status = test_workspace_status();
        let options = vec![model_option()];
        let status = StatusLine {
            config_options: &options,
            exit_confirmation_active: true,
            ..status_line(&workspace_status, &settings)
        };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("Ctrl-C again to exit"), "should show exit warning, got: {text}");
        assert!(!text.contains("test-agent"), "should not show agent name during exit confirmation, got: {text}");
    }

    #[test]
    fn text_segment_with_style() {
        let settings = ResolvedStatusLineSettings {
            separator: " · ".to_string(),
            left: vec![StatusLineSegmentConfig::Text {
                value: "hello".to_string(),
                style: Some(StatusLineStyle::Warning),
            }],
            right: vec![],
        };
        let workspace_status = WorkspaceStatus::new("~/code/foo", None);
        let status = StatusLine { agent_name: "test", content_padding: 0, ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((80, 24));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("hello"), "should render text segment, got: {text}");
    }

    #[test]
    fn zero_width_no_panic() {
        let settings = default_settings();
        let workspace_status = test_workspace_status();
        let status = StatusLine { agent_name: "test", ..status_line(&workspace_status, &settings) };

        let context = ViewContext::new((0, 24));
        let frame = status.render(&context);
        assert!(!frame.lines().is_empty(), "should produce at least one line even at width 0");
    }

    #[test]
    fn truncate_text_returns_short_input_unchanged() {
        assert_eq!(truncate_text("short", Some(10)), "short");
    }

    #[test]
    fn truncate_text_elides_long_input() {
        let result = truncate_text("a very long directory path that exceeds the limit", Some(10));
        assert!(display_width_text(&result) <= 10, "truncated text should fit within max_width");
        assert!(result.ends_with('…'), "truncated text should end with ellipsis, got: {result}");
    }
}
