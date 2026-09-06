use crate::view::diff::render_diff;
use crate::view::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::view::wrap::{as_u16, truncate_to_width, wrap_line};
use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::item_view::indent_lines;
use super::progress_indicator::spinner_frame;
use super::tool_calls::{SUB_AGENT_VISIBLE_TOOL_LIMIT, SubAgentState, ToolCall, ToolStatus};

const MAX_TOOL_ARG_WIDTH: usize = 200;

/// One tool call's rendered rows: status line, diff preview, and the tree of
/// sub-agents beneath it.
pub(crate) fn tool_lines(
    tool: &ToolCall,
    content_width: u16,
    padding: usize,
    spinner_tick: usize,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let parsed_command = tool.bash_command();
    let bash_command = visible_bash_command(parsed_command.as_deref(), tool.display_value.as_deref(), &tool.status);
    let detail = bash_command.map_or_else(
        || tool_detail(tool.display_value.as_deref(), &tool.raw_input, &tool.status),
        |command| bash_tool_detail(command, tool.display_value.as_deref(), &tool.status),
    );
    let prefix = Line::from(vec![
        Span::raw(" ".repeat(padding)),
        status_glyph(&tool.status, spinner_tick, theme),
        Span::raw(" "),
        Span::styled(
            display_title(&tool.title, tool.is_bash(), &tool.status).to_owned(),
            Style::new().fg(theme.text_primary),
        ),
    ]);
    let suffix = tool_suffix(detail, &tool.status, theme);
    let mut lines = tool_line(prefix, suffix, bash_command, content_width, padding + 2, theme, highlighter);
    if matches!(tool.status, ToolStatus::Success)
        && let Some(preview) = &tool.diff
    {
        lines.extend(indent_lines(render_diff(preview, content_width, theme, highlighter), padding));
    }
    if !tool.sub_agents.is_empty() {
        lines.push(Line::default());
        lines.extend(sub_agent_tree_lines(&tool.sub_agents, content_width, spinner_tick, padding, theme, highlighter));
    }
    lines
}

/// Tree of sub-agents beneath a spawning tool, each with its recent tool calls.
fn sub_agent_tree_lines(
    sub_agents: &[SubAgentState],
    content_width: u16,
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let pad = " ".repeat(padding);
    let muted = Style::new().fg(theme.muted);
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (index, agent) in sub_agents.iter().enumerate() {
        if index > 0 {
            lines.push(Line::raw(format!("{pad}  ")));
        }

        let done = if agent.done { ToolStatus::Success } else { ToolStatus::Running };
        lines.push(Line::from(vec![
            Span::raw(format!("{pad}  ")),
            status_glyph(&done, spinner_tick, theme),
            Span::raw(format!(" {}", agent.agent_name)),
        ]));

        // Only the most recent calls are shown; older ones collapse into a count.
        let hidden = agent.tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOL_LIMIT);
        if hidden > 0 {
            lines.push(Line::styled(format!("{pad}  … {hidden} earlier tool calls"), muted));
        }

        let visible: Vec<_> = agent.tool_calls.iter().skip(hidden).collect();
        for (index, tool) in visible.iter().enumerate() {
            let branch = if index + 1 == visible.len() { "  └─ " } else { "  ├─ " };
            let parsed_command = tool.bash_command();
            let bash_command =
                visible_bash_command(parsed_command.as_deref(), tool.display_value.as_deref(), &tool.status);
            let detail = bash_command.map_or_else(
                || tool_detail(tool.display_value.as_deref(), &tool.arguments, &tool.status),
                |command| bash_tool_detail(command, tool.display_value.as_deref(), &tool.status),
            );
            let prefix = Line::from(vec![
                Span::raw(format!("{pad}{branch}")),
                status_glyph(&tool.status, spinner_tick, theme),
                Span::raw(format!(" {}", display_title(&tool.name, tool.is_bash(), &tool.status))),
            ]);
            let suffix = tool_suffix(detail, &tool.status, theme);
            lines.extend(tool_line(prefix, suffix, bash_command, content_width, padding + 6, theme, highlighter));
        }
    }

    lines
}

/// Status marker for a tool call: a spinner while running, then a verdict.
fn status_glyph(status: &ToolStatus, spinner_tick: usize, theme: &Theme) -> Span<'static> {
    let (glyph, color) = match status {
        ToolStatus::Running => (spinner_frame(spinner_tick), theme.info),
        ToolStatus::Success => ("✓", theme.success),
        ToolStatus::Error(_) => ("✗", theme.error),
    };
    Span::styled(glyph, Style::new().fg(color))
}

/// The trailing detail on a tool line: the agent's own summary when it supplied
/// one, otherwise the raw arguments. A running tool shows nothing until it has
/// something to report.
fn tool_detail(display_value: Option<&str>, raw_input: &str, status: &ToolStatus) -> String {
    match display_value.filter(|value| !value.is_empty()) {
        Some(value) => format!(" ({value})"),
        None if matches!(status, ToolStatus::Running) => String::new(),
        None => format!(" {}", truncate_to_width(raw_input, MAX_TOOL_ARG_WIDTH)),
    }
}

fn visible_bash_command<'a>(
    command: Option<&'a str>,
    display_value: Option<&str>,
    status: &ToolStatus,
) -> Option<&'a str> {
    command.filter(|_| !matches!(status, ToolStatus::Running) || display_value.is_some_and(|value| !value.is_empty()))
}

fn bash_tool_detail(command: &str, display_value: Option<&str>, status: &ToolStatus) -> String {
    if matches!(status, ToolStatus::Running) {
        return String::new();
    }
    let Some(value) = display_value.filter(|value| !value.is_empty() && *value != command) else {
        return String::new();
    };
    value.rfind(" (exit ").map_or_else(|| format!(" ({value})"), |index| format!(" {}", &value[index + 1..]))
}

/// A finished bash call reads as a completed action: the agent's "Run command"
/// title renders as "Ran" once the command is no longer running.
fn display_title<'a>(title: &'a str, is_bash: bool, status: &ToolStatus) -> &'a str {
    if is_bash && !matches!(status, ToolStatus::Running) && title == "Run command" {
        "Ran"
    } else {
        title
    }
}

/// The muted detail and, on failure, the error cause that trail a tool line.
fn tool_suffix(detail: String, status: &ToolStatus, theme: &Theme) -> Vec<Span<'static>> {
    let mut suffix = vec![Span::styled(detail, Style::new().fg(theme.muted))];
    if let ToolStatus::Error(cause) = status {
        suffix.push(Span::styled(format!(" {cause}"), Style::new().fg(theme.error)));
    }
    suffix
}

/// One tool line — glyph, title, highlighted bash command, detail — wrapped,
/// with continuation rows aligned under the title.
fn tool_line(
    mut line: Line<'static>,
    suffix: Vec<Span<'static>>,
    command: Option<&str>,
    width: u16,
    continuation_padding: usize,
    theme: &Theme,
    highlighter: &mut SyntaxHighlighter,
) -> Vec<Line<'static>> {
    let command_lines = command.map(|command| highlighter.highlight(command, "bash", theme));
    if let Some(command_lines) = &command_lines
        && let Some(first) = command_lines.first()
    {
        line.push_span(Span::raw(" "));
        line.spans.extend(styled_code_line(first.clone(), theme).spans);
    }
    line.spans.extend(suffix);

    let mut lines = wrap_line(line, width);
    if let Some(command_lines) = command_lines {
        let continuation_width = width.saturating_sub(as_u16(continuation_padding));
        for command_line in command_lines.iter().skip(1) {
            lines.extend(wrap_line(styled_code_line(command_line.clone(), theme), continuation_width));
        }
    }
    if continuation_padding > 0 {
        let prefix = " ".repeat(continuation_padding);
        for line in lines.iter_mut().skip(1) {
            line.spans.insert(0, Span::raw(prefix.clone()));
        }
    }
    lines
}

/// Token colors only: the command sits directly on the terminal's own
/// background rather than a code block behind it.
fn styled_code_line(mut line: Line<'static>, theme: &Theme) -> Line<'static> {
    line.style = line.style.patch(Style::new().fg(theme.code_fg));
    line
}
