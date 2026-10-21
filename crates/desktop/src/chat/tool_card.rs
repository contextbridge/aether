use acp_utils::conversation::{SubAgentState, ToolCall, ToolStatus};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{AnyElement, App, ClickEvent, FontWeight, Hsla, Window, div, prelude::*};

/// A tool call as a card: its status, name and a one-line summary, with any file changes it
/// made below and the tree of sub-agents it spawned. Clicking the header shows or hides its
/// input and output.
///
/// The header is tagged `tool-card-<index>`, the input and output `tool-details-<index>` and the
/// file changes `tool-changes-<index>`.
pub(crate) fn tool_card(
    tool: &ToolCall,
    index: usize,
    expanded: bool,
    theme: &UiTheme,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let header = div()
        .id(("tool-card", index))
        .debug_selector(move || format!("tool-card-{index}"))
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .cursor_pointer()
        .hover(|header| header.bg(theme.colors.surface_hover))
        .on_click(on_toggle)
        .child(status_glyph(tool.status, theme))
        .child(div().flex_none().font_weight(FontWeight::SEMIBOLD).child(title(tool)))
        .child(div().flex_1().min_w_0().truncate().text_color(theme.colors.text_muted).children(summary(tool)))
        .child(div().flex_none().text_color(theme.colors.text_muted).child(if expanded { "▾" } else { "▸" }));
    let diffs: Vec<_> = tool.diffs().collect();
    div()
        .flex()
        .flex_col()
        .rounded_md()
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.surface)
        .text_sm()
        .child(header)
        .when(expanded, |card| card.child(details(tool, index, theme)))
        .when(!diffs.is_empty(), |card| card.child(changes(&diffs, index, expanded, theme)))
        .when(!tool.sub_agents.is_empty(), |card| card.child(sub_agent_tree(&tool.sub_agents, theme)))
        .into_any_element()
}

/// How many lines of a file change a collapsed card shows.
const COLLAPSED_PATCH_LINES: usize = 12;
/// How many of a sub-agent's latest tool calls its tree shows.
const SUB_AGENT_VISIBLE_TOOLS: usize = 3;

fn title(tool: &ToolCall) -> String {
    Some(tool.title()).filter(|title| !title.is_empty()).unwrap_or("Tool call").to_owned()
}

/// The command a bash call runs, or else the agent's own summary of the call.
fn summary(tool: &ToolCall) -> Option<String> {
    tool.bash_command()
        .and_then(|command| command.lines().next())
        .map(|command| format!("$ {command}"))
        .or_else(|| tool.display_value().filter(|value| !value.is_empty()).map(str::to_owned))
}

fn details(tool: &ToolCall, index: usize, theme: &UiTheme) -> impl IntoElement {
    let output: Vec<String> = tool.content().iter().filter_map(content_text).collect();
    div()
        .debug_selector(move || format!("tool-details-{index}"))
        .flex()
        .flex_col()
        .gap_2()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(theme.colors.border)
        .child(section("Input", tool.raw_input(), theme))
        .when(!output.is_empty(), |details| details.child(section("Output", output.join("\n"), theme)))
}

fn section(label: &'static str, text: String, theme: &UiTheme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_xs().text_color(theme.colors.text_muted).child(label))
        .child(div().p_2().rounded_sm().bg(theme.colors.canvas).child(text))
}

fn content_text(content: &acp::ToolCallContent) -> Option<String> {
    match content {
        acp::ToolCallContent::Content(content) => match &content.content {
            acp::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The file changes a tool made, as patches when the agent sent them and otherwise as the paths
/// it touched.
fn changes(diffs: &[&acp::Diff], index: usize, expanded: bool, theme: &UiTheme) -> impl IntoElement {
    let mut changes = div()
        .debug_selector(move || format!("tool-changes-{index}"))
        .flex()
        .flex_col()
        .px_3()
        .py_2()
        .border_t_1()
        .border_color(theme.colors.border);
    for diff in diffs {
        changes = match &diff.patch {
            Some(patch) => changes.child(patch_view(&patch.text, expanded, theme)),
            None => changes.children(diff.changes.iter().map(|change| {
                div().text_color(theme.colors.text_secondary).child(change_label(change)).into_any_element()
            })),
        };
    }
    changes
}

fn patch_view(patch: &str, expanded: bool, theme: &UiTheme) -> impl IntoElement {
    let lines: Vec<&str> = patch.lines().filter(|line| !is_patch_header(line)).collect();
    let shown = if expanded { lines.len() } else { lines.len().min(COLLAPSED_PATCH_LINES) };
    let hidden = lines.len() - shown;
    let mut view = div().flex().flex_col();
    for line in &lines[..shown] {
        let (text, background) = patch_line_colors(line, theme);
        view = view.child(div().px_1().text_color(text).when_some(background, Styled::bg).child(if line.is_empty() {
            " ".to_owned()
        } else {
            (*line).to_owned()
        }));
    }
    view.when(hidden > 0, |view| {
        view.child(div().pt_1().text_color(theme.colors.text_muted).child(format!("… {hidden} more lines")))
    })
}

/// Lines git writes before each file's hunks. `+++ b/<path>` stays, to name the file.
fn is_patch_header(line: &str) -> bool {
    ["diff --git ", "index ", "--- ", "new file mode", "deleted file mode", "similarity index", "rename "]
        .iter()
        .any(|prefix| line.starts_with(prefix))
}

fn patch_line_colors(line: &str, theme: &UiTheme) -> (Hsla, Option<Hsla>) {
    let tint = |mut color: Hsla| {
        color.a = 0.12;
        color
    };
    if line.starts_with("+++ ") {
        (theme.colors.text, None)
    } else if line.starts_with("@@") {
        (theme.colors.text_muted, None)
    } else if line.starts_with('+') {
        (theme.colors.positive, Some(tint(theme.colors.positive)))
    } else if line.starts_with('-') {
        (theme.colors.destructive, Some(tint(theme.colors.destructive)))
    } else {
        (theme.colors.text_secondary, None)
    }
}

fn change_label(change: &acp::DiffChange) -> String {
    match &change.operation {
        acp::DiffChangeOperation::Add(change) => format!("A {}", change.path.0.display()),
        acp::DiffChangeOperation::Delete(change) => format!("D {}", change.path.0.display()),
        acp::DiffChangeOperation::Modify(change) => format!("M {}", change.path.0.display()),
        acp::DiffChangeOperation::Move(change) => {
            format!("R {} → {}", change.old_path.0.display(), change.path.0.display())
        }
        acp::DiffChangeOperation::Copy(change) => {
            format!("C {} → {}", change.old_path.0.display(), change.path.0.display())
        }
        _ => "Unknown file change".to_owned(),
    }
}

/// Each sub-agent with its latest tool calls, older ones folded into a count.
fn sub_agent_tree(sub_agents: &[SubAgentState], theme: &UiTheme) -> impl IntoElement {
    let mut tree = div().flex().flex_col().gap_1().px_3().py_2().border_t_1().border_color(theme.colors.border);
    for agent in sub_agents {
        let status = if agent.done { ToolStatus::Success } else { ToolStatus::Running };
        tree = tree.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_glyph(status, theme))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(agent.agent_name.clone())),
        );
        let hidden = agent.tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOLS);
        if hidden > 0 {
            tree = tree.child(
                div().pl_6().text_color(theme.colors.text_muted).child(format!("… {hidden} earlier tool calls")),
            );
        }
        for tool in &agent.tool_calls[hidden..] {
            tree = tree.child(
                div()
                    .pl_6()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(status_glyph(tool.status, theme))
                    .child(div().flex_none().child(title(tool)))
                    .child(div().min_w_0().truncate().text_color(theme.colors.text_muted).children(summary(tool))),
            );
        }
    }
    tree
}

fn status_glyph(status: ToolStatus, theme: &UiTheme) -> impl IntoElement {
    let (glyph, color) = match status {
        ToolStatus::Running => ("●", theme.colors.info),
        ToolStatus::Success => ("✓", theme.colors.positive),
        ToolStatus::Failed => ("✗", theme.colors.destructive),
        ToolStatus::Cancelled => ("⊘", theme.colors.text_muted),
    };
    div().flex_none().text_color(color).child(glyph)
}
