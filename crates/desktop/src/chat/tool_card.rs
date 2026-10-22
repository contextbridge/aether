use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_CONTROL, ROW_HEIGHT, rem};
use crate::ui::{CODE_FONT, Icon, IconName, Typography, TypographyExt};
use acp_utils::conversation::{SubAgentState, ToolCall, ToolStatus};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{AnyElement, App, ClickEvent, Hsla, Role, Window, div, prelude::*};
use std::collections::HashSet;
use std::path::Path;

/// A compact tool activity row. Clicking it unfolds input, output and file changes along a
/// single indented guide. Sub-agent activity stays visible while the details are folded.
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
    let colors = theme.colors;
    let chevron = if expanded { IconName::ChevronDown } else { IconName::ChevronRight };
    let mut disclosure_color = colors.text_muted;
    if !expanded {
        disclosure_color.a = 0.0;
    }
    let diffs: Vec<_> = tool.diffs().collect();
    let header = div()
        .id(("tool-card", index))
        .debug_selector(move || format!("tool-card-{index}"))
        .role(Role::Button)
        .aria_label(title(tool))
        .aria_expanded(expanded)
        .focusable()
        .tab_stop(true)
        .h(rem(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap_2()
        .px_1()
        .rounded(rem(RADIUS_CONTROL))
        .cursor_pointer()
        .text_color(disclosure_color)
        .hover(move |header| header.bg(colors.surface_hover).text_color(colors.text_muted))
        .focus_visible(move |header| header.bg(colors.surface_hover).text_color(colors.text_muted))
        .on_click(on_toggle)
        .child(status_icon(tool.status, tool.id(), theme))
        .child(
            div()
                .debug_selector(move || format!("tool-title-{index}"))
                .min_w_0()
                .truncate()
                .typography(Typography::Label)
                .text_color(colors.text_secondary)
                .child(title(tool)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .typography(Typography::Small)
                .text_color(if tool.status == ToolStatus::Failed { colors.destructive } else { colors.text_muted })
                .when(tool.bash_command().is_some() || !diffs.is_empty(), |summary| summary.font_family(CODE_FONT))
                .children(summary(tool)),
        )
        .when(!diffs.is_empty(), |header| header.child(change_summary(&diffs, index, theme)))
        .child(
            div()
                .debug_selector(move || format!("tool-disclosure-{index}"))
                .flex_none()
                .size(rem(ICON_SIZE_SMALL))
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(chevron, colors.text_muted).size(ICON_SIZE_SMALL).inherit_color()),
        );
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .child(header)
        .when(expanded || !tool.sub_agents.is_empty(), |row| {
            row.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .ml_2()
                    .mt_1()
                    .mb_2()
                    .pl_4()
                    .pr_2()
                    .py_1()
                    .border_l_1()
                    .border_color(colors.border)
                    .when(expanded, |body| body.child(details(tool, index, theme)))
                    .when(expanded && !diffs.is_empty(), |body| body.child(changes(&diffs, index, theme)))
                    .when(!tool.sub_agents.is_empty(), |body| body.child(sub_agent_tree(&tool.sub_agents, theme))),
            )
        })
        .into_any_element()
}

/// How many of a sub-agent's latest tool calls its tree shows.
const SUB_AGENT_VISIBLE_TOOLS: usize = 3;

fn title(tool: &ToolCall) -> String {
    Some(tool.title()).filter(|title| !title.is_empty()).unwrap_or("Tool call").to_owned()
}

/// An error, command, display value or changed path, in order of relevance.
fn summary(tool: &ToolCall) -> Option<String> {
    if tool.status == ToolStatus::Failed {
        let error = tool
            .content()
            .iter()
            .filter_map(content_text)
            .find_map(|text| text.lines().find(|line| !line.trim().is_empty()).map(str::to_owned));
        if error.is_some() {
            return error;
        }
    }
    tool.bash_command()
        .and_then(|command| command.lines().next())
        .map(|command| format!("$ {command}"))
        .or_else(|| tool.display_value().filter(|value| !value.is_empty()).map(str::to_owned))
        .or_else(|| {
            tool.diffs().flat_map(|diff| &diff.changes).find_map(change_path).map(|path| path.display().to_string())
        })
}

fn details(tool: &ToolCall, index: usize, theme: &UiTheme) -> impl IntoElement {
    let input = tool.raw_input();
    let output = tool.content().iter().filter_map(content_text).collect::<Vec<_>>().join("\n");
    div()
        .debug_selector(move || format!("tool-details-{index}"))
        .flex()
        .flex_col()
        .gap_3()
        .when(!input.is_empty(), |details| details.child(section("Input", input, theme)))
        .when(!output.is_empty(), |details| details.child(section("Output", output, theme)))
}

fn section(label: &'static str, text: String, theme: &UiTheme) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().typography(Typography::Label).text_color(theme.colors.text_muted).child(label))
        .child(
            div()
                .text_color(theme.colors.text_secondary)
                .font_family(CODE_FONT)
                .typography(Typography::Small)
                .child(text),
        )
}

fn change_summary(diffs: &[&acp::Diff], index: usize, theme: &UiTheme) -> impl IntoElement {
    let mut added = 0;
    let mut removed = 0;
    for patch in diffs.iter().filter_map(|diff| diff.patch.as_ref()) {
        let mut in_hunk = false;
        for line in patch.text.lines() {
            if line.starts_with("@@") {
                in_hunk = true;
            } else if line.starts_with("diff --git ") {
                in_hunk = false;
            } else if in_hunk && line.starts_with('+') {
                added += 1;
            } else if in_hunk && line.starts_with('-') {
                removed += 1;
            }
        }
    }
    let files = diffs.iter().flat_map(|diff| &diff.changes).filter_map(change_path).collect::<HashSet<_>>().len();
    div()
        .debug_selector(move || format!("tool-change-summary-{index}"))
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .typography(Typography::Caption)
        .text_color(theme.colors.text_muted)
        .when(files > 1 || (added == 0 && removed == 0), |summary| {
            let label = match files {
                0 => "File changes".to_owned(),
                1 => "1 file".to_owned(),
                _ => format!("{files} files"),
            };
            summary.child(label)
        })
        .when(added > 0, |summary| summary.child(div().text_color(theme.colors.positive).child(format!("+{added}"))))
        .when(removed > 0, |summary| {
            summary.child(div().text_color(theme.colors.destructive).child(format!("−{removed}")))
        })
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
fn changes(diffs: &[&acp::Diff], index: usize, theme: &UiTheme) -> impl IntoElement {
    let mut changes = div()
        .debug_selector(move || format!("tool-changes-{index}"))
        .flex()
        .flex_col()
        .gap_2()
        .font_family(CODE_FONT)
        .typography(Typography::Small);
    for diff in diffs {
        changes = match &diff.patch {
            Some(patch) => changes.child(patch_view(&patch.text, theme)),
            None => changes.children(diff.changes.iter().map(|change| {
                div().text_color(theme.colors.text_secondary).child(change_label(change)).into_any_element()
            })),
        };
    }
    changes
}

fn patch_view(patch: &str, theme: &UiTheme) -> impl IntoElement {
    let lines: Vec<&str> = patch.lines().filter(|line| !is_patch_header(line)).collect();
    let mut view = div().flex().flex_col();
    for line in lines {
        let (text, background) = patch_line_colors(line, theme);
        view = view.child(div().px_1().text_color(text).when_some(background, Styled::bg).child(if line.is_empty() {
            " ".to_owned()
        } else {
            line.to_owned()
        }));
    }
    view
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

fn change_path(change: &acp::DiffChange) -> Option<&Path> {
    match &change.operation {
        acp::DiffChangeOperation::Add(change)
        | acp::DiffChangeOperation::Delete(change)
        | acp::DiffChangeOperation::Modify(change) => Some(&change.path.0),
        acp::DiffChangeOperation::Move(change) | acp::DiffChangeOperation::Copy(change) => Some(&change.path.0),
        _ => None,
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
    let mut tree = div().flex().flex_col().gap_1().typography(Typography::Small);
    for agent in sub_agents {
        let status = if agent.done { ToolStatus::Success } else { ToolStatus::Running };
        tree = tree.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(status_icon(status, &agent.task_id, theme))
                .child(div().typography(Typography::Label).child(agent.agent_name.clone())),
        );
        let hidden = agent.tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOLS);
        if hidden > 0 {
            tree = tree
                .child(div().pl_6().text_color(theme.colors.text_muted).child(format!("{hidden} earlier tool calls")));
        }
        for tool in &agent.tool_calls[hidden..] {
            tree = tree.child(
                div()
                    .pl_6()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(status_icon(tool.status, &format!("{}-{}", agent.task_id, tool.id()), theme))
                    .child(div().flex_none().child(title(tool)))
                    .child(div().min_w_0().truncate().text_color(theme.colors.text_muted).children(summary(tool))),
            );
        }
    }
    tree
}

fn status_icon(status: ToolStatus, id: &str, theme: &UiTheme) -> Icon {
    let (icon, color) = match status {
        ToolStatus::Running => (IconName::LoaderCircle, theme.colors.info),
        ToolStatus::Success => (IconName::Check, theme.colors.text_muted),
        ToolStatus::Failed => (IconName::CircleX, theme.colors.destructive),
        ToolStatus::Cancelled => (IconName::CircleSlash, theme.colors.text_muted),
    };
    Icon::new(icon, color)
        .size(ICON_SIZE_SMALL)
        .when(status == ToolStatus::Running, |icon| icon.spin(format!("tool-spinner-{id}")))
}
