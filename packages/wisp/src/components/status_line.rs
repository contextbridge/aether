use crate::components::context_bar::{context_bar, context_color};
use crate::components::reasoning_bar::{reasoning_bar, reasoning_color};
use crate::workspace_status::WorkspaceStatus;
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{
    self as acp, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions,
};
use tui::{Color, FitOptions, Frame, Line, ViewContext};
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
}

impl StatusLine<'_> {
    pub fn render(&self, context: &ViewContext) -> Frame {
        let left = render_left(self.workspace_status, context, self.content_padding);
        let right = render_right(self, context);
        let width = context.size.width as usize;

        let lines = if left.display_width() + right.display_width() <= width {
            vec![join_aligned(left, &right, width)]
        } else {
            vec![left, align_left(&right, self.content_padding)]
        };

        Frame::new(lines).fit(context.size.width, FitOptions::truncate())
    }
}

fn render_left(status: &WorkspaceStatus, context: &ViewContext, content_padding: usize) -> Line {
    let mut line = Line::default();
    line.push_text(" ".repeat(content_padding));
    line.push_styled(status.display_dir.as_str(), context.theme.secondary());
    if let Some(ref git_ref) = status.git_ref {
        line.push_styled(" · ", context.theme.text_secondary());
        line.push_styled(git_ref.as_str(), context.theme.success());
    }
    line
}

fn render_right(status: &StatusLine<'_>, context: &ViewContext) -> Line {
    let sep = context.theme.text_secondary();
    let mode_text = extract_mode_display(status.config_options);
    let model_summary = extract_model_display(status.config_options);
    let reasoning_effort = extract_reasoning_effort(status.config_options);

    let mut line = Line::default();

    if status.exit_confirmation_active {
        line.push_styled("Ctrl-C again to exit", context.theme.warning());
    } else {
        line.push_styled(status.agent_name, context.theme.info());

        if let Some(ref mode) = mode_text {
            push_separator(&mut line, sep);
            line.push_styled(mode.clone(), context.theme.secondary());
        }

        if let Some(ref model) = model_summary {
            push_separator(&mut line, sep);
            line.push_styled(model.clone(), context.theme.success());
        }
    }

    let reasoning_levels = extract_reasoning_levels(status.config_options);
    if model_summary.is_some() && !reasoning_levels.is_empty() {
        push_separator(&mut line, sep);
        line.push_styled(
            reasoning_bar(reasoning_effort, reasoning_levels.len()),
            reasoning_color(reasoning_effort, reasoning_levels.len(), &context.theme),
        );
    }

    if let Some(usage) = status.context_usage {
        push_separator(&mut line, sep);
        line.push_styled(context_bar(usage), context_color(usage, &context.theme));
    }

    if !status.waiting_for_response && status.unhealthy_server_count > 0 {
        let count = status.unhealthy_server_count;
        let msg = if count == 1 { "1 server needs auth".to_string() } else { format!("{count} servers unhealthy") };
        push_separator(&mut line, sep);
        line.push_styled(msg, context.theme.warning());
    }

    line
}

fn join_aligned(mut left: Line, right: &Line, width: usize) -> Line {
    let padding = width.saturating_sub(left.display_width() + right.display_width());
    left.push_text(" ".repeat(padding));
    left.append_line(right);
    left
}

fn align_left(right: &Line, content_padding: usize) -> Line {
    let mut line = Line::default();
    line.push_text(" ".repeat(content_padding));
    line.append_line(right);
    line
}

fn push_separator(line: &mut Line, color: Color) {
    if !line.is_empty() {
        line.push_styled(" · ", color);
    }
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
    use crate::workspace_status::WorkspaceStatus;

    fn test_workspace_status() -> WorkspaceStatus {
        WorkspaceStatus::new("~/code/aether-2", Some("main".to_string()))
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

    fn status_line() -> StatusLine<'static> {
        static WORKSPACE_STATUS: std::sync::LazyLock<WorkspaceStatus> = std::sync::LazyLock::new(test_workspace_status);

        StatusLine {
            workspace_status: &WORKSPACE_STATUS,
            agent_name: "test-agent",
            config_options: &[],
            context_usage: None,
            waiting_for_response: false,
            unhealthy_server_count: 0,
            content_padding: DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: false,
        }
    }

    #[test]
    fn reasoning_bar_hidden_without_reasoning_option() {
        let options = vec![model_option()];
        let status = StatusLine { config_options: &options, ..status_line() };

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
        let status = StatusLine { config_options: &options, ..status_line() };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("medium"), "reasoning bar should use current reasoning effort as its label, got: {text}");
        assert!(!text.contains("reasoning"), "reasoning bar should not use a generic reasoning label, got: {text}");
    }

    #[test]
    fn wraps_right_side_onto_second_line_when_too_narrow() {
        let options = vec![model_option(), reasoning_option()];
        let status = StatusLine { config_options: &options, ..status_line() };

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
        let status = StatusLine { config_options: &options, ..status_line() };

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
}
