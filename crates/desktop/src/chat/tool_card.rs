use super::code::highlight;
use crate::ui::UiTheme;
use crate::ui::tokens::{ICON_SIZE_SMALL, ROW_HEIGHT, rem};
use crate::ui::{CODE_FONT, Disclosure, Icon, IconName, Typography, TypographyExt};
use acp_utils::conversation::{SubAgentState, ToolCall, ToolStatus, diff_change_label};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_gpui::DiffPreviewCache;
use clankerdiff_gpui::style::stat_badges;
use clankerdiff_syntax::{LanguageHint, SyntaxHighlighter};
use clankerdiff_theme::ReviewTheme;
use gpui::{AnyElement, App, ClickEvent, StyledText, Window, div, prelude::*};
use std::collections::HashSet;
use std::path::Path;

/// Renders tool activity with the transcript's shared syntax and parsed-patch caches.
pub(crate) struct ToolRenderer<'a> {
    review: &'a ReviewTheme,
    highlighter: &'a mut SyntaxHighlighter,
    patches: &'a mut DiffPreviewCache,
}

impl<'a> ToolRenderer<'a> {
    pub(crate) fn new(
        review: &'a ReviewTheme,
        highlighter: &'a mut SyntaxHighlighter,
        patches: &'a mut DiffPreviewCache,
    ) -> Self {
        Self { review, highlighter, patches }
    }

    /// A compact tool activity row. Clicking it unfolds input, output and file changes along a
    /// single indented guide. Sub-agent activity stays visible while the details are folded.
    pub(crate) fn render(
        &mut self,
        tool: &ToolCall,
        index: usize,
        expanded: bool,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let theme = &UiTheme::new(self.review);
        let colors = theme.colors;
        let diffs: Vec<_> = tool.diffs().collect();
        let header = Disclosure::new(("tool-card", index), expanded, *theme)
            .h(rem(ROW_HEIGHT))
            .overflow_hidden()
            .on_toggle(on_toggle)
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
                    .children(self.summary(tool)),
            )
            .when(!diffs.is_empty(), |header| header.child(self.change_summary(&diffs, index, theme)));
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
                        .when(expanded, |body| body.child(self.details(tool, index, theme)))
                        .when(expanded && !diffs.is_empty(), |body| body.child(self.changes(&diffs, index, theme)))
                        .when(!tool.sub_agents.is_empty(), |body| {
                            body.child(self.sub_agent_tree(&tool.sub_agents, theme))
                        }),
                )
            })
            .into_any_element()
    }

    /// An error, highlighted command, display value or changed path, in order of relevance.
    fn summary(&mut self, tool: &ToolCall) -> Option<AnyElement> {
        if tool.status == ToolStatus::Failed {
            let error =
                tool.content().iter().filter_map(content_text).find_map(|text| first_line(&text).map(str::to_owned));
            if let Some(error) = error {
                return Some(error.into_any_element());
            }
        }
        if let Some(command) = tool.bash_command()
            && let Some(first) = command.lines().next()
        {
            let highlights = highlight(command, LanguageHint::Id("bash"), &self.review.syntax, self.highlighter)
                .into_iter()
                .filter_map(|(range, style)| {
                    (range.start < first.len()).then(|| (range.start + 2..range.end.min(first.len()) + 2, style))
                })
                .collect::<Vec<_>>();
            return Some(StyledText::new(format!("$ {first}")).with_highlights(highlights).into_any_element());
        }
        tool.display_value()
            .and_then(first_line)
            .map(str::to_owned)
            .or_else(|| {
                tool.diffs().flat_map(|diff| &diff.changes).find_map(change_path).map(|path| path.display().to_string())
            })
            .map(IntoElement::into_any_element)
    }

    fn details(&mut self, tool: &ToolCall, index: usize, theme: &UiTheme) -> gpui::Div {
        let command = tool.bash_command().map(|command| {
            let highlights = highlight(command, LanguageHint::Id("bash"), &self.review.syntax, self.highlighter);
            section("Command", StyledText::new(command.to_owned()).with_highlights(highlights), theme)
        });
        let input = tool.raw_input();
        let output = tool.content().iter().filter_map(content_text).collect::<Vec<_>>().join("\n");
        div()
            .debug_selector(move || format!("tool-details-{index}"))
            .flex()
            .flex_col()
            .gap_3()
            .children(command)
            .when(!input.is_empty(), |details| details.child(section("Input", input, theme)))
            .when(!output.is_empty(), |details| details.child(section("Output", output, theme)))
    }

    /// The file changes a tool made, as patches when the agent sent them and otherwise as the paths
    /// it touched.
    fn changes(&mut self, diffs: &[&acp::Diff], index: usize, theme: &UiTheme) -> gpui::Div {
        let mut changes = div()
            .debug_selector(move || format!("tool-changes-{index}"))
            .flex()
            .flex_col()
            .gap_2()
            .font_family(CODE_FONT)
            .typography(Typography::Small);
        for diff in diffs {
            changes = match diff.patch.as_ref().filter(|patch| !patch.text.trim().is_empty()) {
                Some(patch) if patch.format == acp::DiffPatchFormat::GitPatch => {
                    changes.child(self.patches.render(&patch.text, self.review, self.highlighter))
                }
                Some(patch) => changes.child(section("Patch", patch.text.clone(), theme)),
                None => changes.children(diff.changes.iter().map(|change| {
                    div().text_color(theme.colors.text_secondary).child(diff_change_label(change)).into_any_element()
                })),
            };
        }
        changes
    }

    /// Lines added and removed, as the patches parse, and the number of files when that says more.
    fn change_summary(&mut self, diffs: &[&acp::Diff], index: usize, theme: &UiTheme) -> impl IntoElement + use<> {
        let (added, removed) = diffs
            .iter()
            .filter_map(|diff| diff.patch.as_ref())
            .filter_map(|patch| self.patches.stats(&patch.text))
            .fold((0, 0), |(added, removed), (more, fewer)| (added + more, removed + fewer));
        let files = diffs.iter().flat_map(|diff| &diff.changes).filter_map(change_path).collect::<HashSet<_>>().len();
        let changed_lines = added > 0 || removed > 0;
        div()
            .debug_selector(move || format!("tool-change-summary-{index}"))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .typography(Typography::Caption)
            .text_color(theme.colors.text_muted)
            .when(files > 1 || !changed_lines, |summary| {
                summary.child(match files {
                    0 => "File changes".to_owned(),
                    1 => "1 file".to_owned(),
                    _ => format!("{files} files"),
                })
            })
            .when(changed_lines, |summary| summary.children(stat_badges(&self.review.diff, added, removed)))
    }

    /// Each sub-agent with its latest tool calls, older ones folded into a count.
    fn sub_agent_tree(&mut self, sub_agents: &[SubAgentState], theme: &UiTheme) -> gpui::Div {
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
                tree = tree.child(
                    div().pl_6().text_color(theme.colors.text_muted).child(format!("{hidden} earlier tool calls")),
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
                        .child(status_icon(tool.status, &format!("{}-{}", agent.task_id, tool.id()), theme))
                        .child(div().flex_none().child(title(tool)))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.colors.text_muted)
                                .when(tool.bash_command().is_some(), |summary| summary.font_family(CODE_FONT))
                                .children(self.summary(tool)),
                        ),
                );
            }
        }
        tree
    }
}

const SUB_AGENT_VISIBLE_TOOLS: usize = 3;

fn title(tool: &ToolCall) -> String {
    first_line(tool.title()).unwrap_or("Tool call").to_owned()
}

/// A tool row holds one line, but agents may send several, such as a whole script as a title.
fn first_line(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|line| !line.is_empty())
}

fn section(label: &'static str, text: impl IntoElement, theme: &UiTheme) -> gpui::Div {
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

fn content_text(content: &acp::ToolCallContent) -> Option<String> {
    match content {
        acp::ToolCallContent::Content(content) => match &content.content {
            acp::ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        },
        _ => None,
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
