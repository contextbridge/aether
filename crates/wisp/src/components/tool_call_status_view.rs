use agent_client_protocol::schema as acp;
use std::collections::HashMap;
use std::path::Path;

use crate::components::sub_agent_tracker::{SUB_AGENT_VISIBLE_TOOL_LIMIT, SubAgentState, SubAgentTracker};
use crate::components::tracked_tool_call::TrackedToolCall;
use tui::BRAILLE_FRAMES as FRAMES;
use tui::{DiffPreview, FitOptions, Frame, Line, Style, ViewContext, render_diff};

pub const MAX_TOOL_ARG_LENGTH: usize = 200;

/// Render a tool call and its sub-agent hierarchy (if any) as a frame.
pub(crate) fn render_tool_tree(
    id: &str,
    tool_calls: &HashMap<String, TrackedToolCall>,
    sub_agents: &SubAgentTracker,
    tick: u16,
    context: &ViewContext,
) -> Frame {
    let has_sub_agents = sub_agents.has_sub_agents(id);

    let mut frames: Vec<Frame> = Vec::new();
    if !has_sub_agents && let Some(tc) = tool_calls.get(id) {
        frames.push(tool_call_view(tc, tick).render(context));
    }

    if let Some(agents) = sub_agents.get(id) {
        for (i, agent) in agents.iter().enumerate() {
            if i > 0 {
                frames.push(Frame::new(vec![Line::default()]));
            }
            frames.push(render_agent_header(agent, tick, context));

            let hidden_count = agent.tool_order.len().saturating_sub(SUB_AGENT_VISIBLE_TOOL_LIMIT);

            if hidden_count > 0 {
                let mut summary = Line::default();
                summary.push_styled(format!("  … {hidden_count} earlier tool calls"), context.theme.muted());
                frames.push(Frame::new(vec![summary]));
            }

            let mut visible = agent
                .tool_order
                .iter()
                .skip(hidden_count)
                .filter_map(|tool_id| agent.tool_calls.get(tool_id))
                .peekable();

            let muted = Style::fg(context.theme.muted());
            while let Some(tc) = visible.next() {
                let is_last = visible.peek().is_none();
                let (head_str, tail_str) = if is_last { ("  └─ ", "     ") } else { ("  ├─ ", "  │  ") };
                let head = Line::with_style(head_str, muted);
                let tail = Line::with_style(tail_str, muted);

                frames.push(tool_call_view(tc, tick).render(context).prefix(&head, &tail));
            }
        }
    }

    Frame::vstack(frames).fit(context.size.width, FitOptions::wrap())
}

pub(crate) fn tool_call_view(tc: &TrackedToolCall, tick: u16) -> ToolCallStatusView<'_> {
    ToolCallStatusView {
        name: &tc.name,
        arguments: &tc.arguments,
        display_value: tc.display_value.as_deref(),
        diff_preview: tc.diff_preview.as_ref(),
        status: &tc.status,
        tick,
    }
}

/// Renders a single tool call status line.
pub struct ToolCallStatusView<'a> {
    pub name: &'a str,
    pub arguments: &'a str,
    pub display_value: Option<&'a str>,
    pub diff_preview: Option<&'a DiffPreview>,
    pub status: &'a ToolCallStatus,
    pub tick: u16,
}

#[derive(Clone)]
pub enum ToolCallStatus {
    Running,
    Success,
    Error(String),
}

impl ToolCallStatusView<'_> {
    pub fn render(&self, context: &ViewContext) -> Frame {
        let (indicator, indicator_color) = match &self.status {
            ToolCallStatus::Running => {
                let frame = FRAMES[self.tick as usize % FRAMES.len()];
                (frame.to_string(), context.theme.info())
            }
            ToolCallStatus::Success => ("✓".to_string(), context.theme.success()),
            ToolCallStatus::Error(_) => ("✗".to_string(), context.theme.error()),
        };

        let mut line = Line::default();
        line.push_styled(indicator, indicator_color);
        line.push_text(" ");
        line.push_text(self.name);

        let display_text = self.display_value.filter(|v| !v.is_empty()).map_or_else(
            || match self.status {
                ToolCallStatus::Running => String::new(),
                _ => format_arguments(self.arguments),
            },
            |v| format!(" ({v})"),
        );
        line.push_styled(display_text, context.theme.muted());

        if let ToolCallStatus::Error(msg) = &self.status {
            line.push_text(" ");
            line.push_styled(msg, context.theme.error());
        }

        let mut lines = vec![line];

        if matches!(self.status, ToolCallStatus::Success)
            && let Some(preview) = self.diff_preview
        {
            lines.extend(render_diff(preview, context));
        }

        Frame::new(lines).fit(context.size.width, FitOptions::wrap())
    }
}

pub(super) fn diff_preview_from_acp(diff: &acp::Diff) -> DiffPreview {
    let old_text = diff.old_text.as_deref().unwrap_or("");
    let lang_hint = Path::new(&diff.path).extension().and_then(|ext| ext.to_str()).unwrap_or("").to_lowercase();
    DiffPreview::compute_trimmed(old_text, &diff.new_text, &lang_hint)
}

fn render_agent_header(agent: &SubAgentState, tick: u16, context: &ViewContext) -> Frame {
    let mut line = Line::default();
    line.push_text("  ");
    if agent.done {
        line.push_styled("✓".to_string(), context.theme.success());
    } else {
        let frame = FRAMES[tick as usize % FRAMES.len()];
        line.push_styled(frame.to_string(), context.theme.info());
    }
    line.push_text(" ");
    line.push_text(&agent.agent_name);
    Frame::new(vec![line])
}

fn format_arguments(arguments: &str) -> String {
    let mut formatted = format!(" {arguments}");
    if formatted.len() > MAX_TOOL_ARG_LENGTH {
        let mut new_len = MAX_TOOL_ARG_LENGTH;
        while !formatted.is_char_boundary(new_len) {
            new_len -= 1;
        }
        formatted.truncate(new_len);
    }
    formatted
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::{DiffTag, SplitDiffRow};

    fn is_context_row(row: &SplitDiffRow) -> bool {
        row.left.as_ref().is_none_or(|c| c.tag == DiffTag::Context)
            && row.right.as_ref().is_none_or(|c| c.tag == DiffTag::Context)
    }

    fn make_large_file(num_lines: usize) -> String {
        (1..=num_lines).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n")
    }

    fn replace_line(text: &str, line_num: usize, replacement: &str) -> String {
        text.lines()
            .enumerate()
            .map(|(i, l)| if i + 1 == line_num { replacement } else { l })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn diff_preview_for_edit_near_end_contains_change() {
        let old = make_large_file(50);
        let new = replace_line(&old, 45, "CHANGED LINE 45");

        let diff = acp::Diff::new("test.rs", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        let has_change = preview.lines.iter().any(|l| l.tag != DiffTag::Context);
        assert!(has_change, "preview must contain the changed lines");
    }

    #[test]
    fn diff_preview_trims_leading_context() {
        let old = make_large_file(50);
        let new = replace_line(&old, 45, "CHANGED LINE 45");

        let diff = acp::Diff::new("test.rs", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        assert!(
            preview.lines.len() <= 10,
            "expected at most ~10 lines (3 context + change + 3 context), got {}",
            preview.lines.len()
        );
    }

    #[test]
    fn diff_preview_start_line_adjusted_after_trim() {
        let old = make_large_file(50);
        let new = replace_line(&old, 45, "CHANGED LINE 45");

        let diff = acp::Diff::new("test.rs", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        let start = preview.start_line.expect("start_line should be set");
        assert!(start >= 42, "start_line should be near the edit (line 45), got {start}");
    }

    #[test]
    fn compute_diff_preview_produces_nonempty_rows_with_correct_pairing() {
        let old = "aaa\nbbb\nccc\n";
        let new = "aaa\nBBB\nccc\n";
        let diff = acp::Diff::new("test.txt", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        assert!(!preview.rows.is_empty(), "rows should not be empty");
        // The replace op should produce a paired row with both left (removed) and right (added)
        let paired = preview.rows.iter().find(|r| r.left.is_some() && r.right.is_some() && !is_context_row(r));
        assert!(paired.is_some(), "should have a paired replace row");
        let row = paired.unwrap();
        assert_eq!(row.left.as_ref().unwrap().tag, DiffTag::Removed);
        assert_eq!(row.right.as_ref().unwrap().tag, DiffTag::Added);
        assert_eq!(row.left.as_ref().unwrap().content, "bbb");
        assert_eq!(row.right.as_ref().unwrap().content, "BBB");
    }

    #[test]
    fn delete_only_produces_rows_with_right_none() {
        let old = "aaa\nbbb\nccc\n";
        let new = "aaa\nccc\n";
        let diff = acp::Diff::new("test.txt", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        let delete_row = preview.rows.iter().find(|r| r.left.as_ref().is_some_and(|c| c.tag == DiffTag::Removed));
        assert!(delete_row.is_some(), "should have a delete row");
        assert!(delete_row.unwrap().right.is_none());
    }

    #[test]
    fn insert_only_produces_rows_with_left_none() {
        let old = "aaa\nccc\n";
        let new = "aaa\nbbb\nccc\n";
        let diff = acp::Diff::new("test.txt", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        let insert_row = preview.rows.iter().find(|r| r.right.as_ref().is_some_and(|c| c.tag == DiffTag::Added));
        assert!(insert_row.is_some(), "should have an insert row");
        assert!(insert_row.unwrap().left.is_none());
    }

    #[test]
    fn context_trimming_applies_consistently_to_lines_and_rows() {
        let old = make_large_file(50);
        let new = replace_line(&old, 25, "CHANGED LINE 25");
        let diff = acp::Diff::new("test.rs", new).old_text(old);
        let preview = diff_preview_from_acp(&diff);

        // Both should be trimmed to roughly the same size (3 context + changes + 3 context)
        assert!(preview.lines.len() <= 10, "lines should be trimmed, got {}", preview.lines.len());
        assert!(preview.rows.len() <= 10, "rows should be trimmed, got {}", preview.rows.len());

        // Both should contain change indicators
        let has_line_change = preview.lines.iter().any(|l| l.tag != DiffTag::Context);
        let has_row_change = preview.rows.iter().any(|r| !is_context_row(r));
        assert!(has_line_change, "lines should contain changes");
        assert!(has_row_change, "rows should contain changes");
    }
}
