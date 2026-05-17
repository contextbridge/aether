use crate::components::context_bar::{context_bar, context_color};
use crate::components::reasoning_bar::{reasoning_bar, reasoning_color};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{
    self as acp, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory, SessionConfigSelectOptions,
};
use tui::{Color, Frame, Line, ViewContext, display_width_text};
use utils::ReasoningEffort;

pub use crate::components::context_bar::ContextUsageDisplay;

#[doc = include_str!("../docs/status_line.md")]
pub struct StatusLine<'a> {
    pub agent_name: &'a str,
    pub config_options: &'a [SessionConfigOption],
    pub context_usage: Option<ContextUsageDisplay>,
    pub waiting_for_response: bool,
    pub unhealthy_server_count: usize,
    pub content_padding: usize,
    pub exit_confirmation_active: bool,
}

impl StatusLine<'_> {
    #[allow(clippy::similar_names)]
    pub fn render(&self, context: &ViewContext) -> Frame {
        let mode_display = extract_mode_display(self.config_options);
        let model_display = extract_model_display(self.config_options);
        let reasoning_effort = extract_reasoning_effort(self.config_options);

        let mut left_line = Line::default();
        let sep = context.theme.text_secondary();

        left_line.push_text(" ".repeat(self.content_padding));

        if self.exit_confirmation_active {
            left_line.push_styled("Ctrl-C again to exit", context.theme.warning());
        } else {
            left_line.push_styled(self.agent_name, context.theme.info());

            if let Some(ref mode) = mode_display {
                left_line.push_styled(" · ", sep);
                left_line.push_styled(mode.as_str(), context.theme.secondary());
            }

            if let Some(ref model) = model_display {
                left_line.push_styled(" · ", sep);
                left_line.push_styled(model.as_str(), context.theme.success());
            }
        }

        let mut right_parts: Vec<(String, Color)> = Vec::new();

        let reasoning_levels = extract_reasoning_levels(self.config_options);
        if model_display.is_some() && !reasoning_levels.is_empty() {
            right_parts.push((
                reasoning_bar(reasoning_effort, reasoning_levels.len()),
                reasoning_color(reasoning_effort, reasoning_levels.len(), &context.theme),
            ));
        }

        if let Some(usage) = self.context_usage {
            if !right_parts.is_empty() {
                right_parts.push((" · ".to_string(), sep));
            }
            right_parts.push((context_bar(usage), context_color(usage, &context.theme)));
        }

        if !self.waiting_for_response && self.unhealthy_server_count > 0 {
            let count = self.unhealthy_server_count;
            let msg = if count == 1 { "1 server needs auth".to_string() } else { format!("{count} servers unhealthy") };
            if !right_parts.is_empty() {
                right_parts.push((" · ".to_string(), sep));
            }
            right_parts.push((msg, context.theme.warning()));
        }

        let width = context.size.width as usize;
        let right_len: usize = right_parts.iter().map(|(s, _)| display_width_text(s)).sum();
        let left_len = left_line.display_width();

        let padding = width.saturating_sub(left_len + right_len);
        left_line.push_text(" ".repeat(padding));
        for (text, color) in right_parts {
            left_line.push_styled(text, color);
        }
        Frame::new(vec![left_line])
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
        let status = StatusLine {
            agent_name: "test-agent",
            config_options: &options,
            context_usage: None,
            waiting_for_response: false,
            unhealthy_server_count: 0,
            content_padding: DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: false,
        };

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
        let status = StatusLine {
            agent_name: "test-agent",
            config_options: &options,
            context_usage: None,
            waiting_for_response: false,
            unhealthy_server_count: 0,
            content_padding: DEFAULT_CONTENT_PADDING,
            exit_confirmation_active: false,
        };

        let context = ViewContext::new((120, 40));
        let frame = status.render(&context);
        let text = frame.lines()[0].plain_text();
        assert!(text.contains("medium"), "reasoning bar should use current reasoning effort as its label, got: {text}");
        assert!(!text.contains("reasoning"), "reasoning bar should not use a generic reasoning label, got: {text}");
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
