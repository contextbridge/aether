use super::code::highlight;
use super::groups::{GroupKind, Work};
use crate::theme::DesktopTheme;
use crate::ui::tokens::{ICON_SIZE_SMALL, ROW_HEIGHT, rem};
use crate::ui::{
    Button, ButtonVariant, CODE_FONT, ControlSize, Disclosure, Icon, IconName, Typography, TypographyExt, UiTheme,
    Waveform,
};
use acp_utils::content::first_line;
use acp_utils::conversation::{ConversationItem, SubAgent, ToolCall, ToolStatus, diff_change_label};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_core::{DiffDocument, LineStat};
use clankerdiff_gpui::DiffPreviewCache;
use clankerdiff_gpui::style::stat_badges;
use clankerdiff_syntax::{LanguageHint, SyntaxHighlighter};
use clankerdiff_ui_core::ReviewTheme;
use gpui::{AnyElement, App, ClickEvent, Div, ElementId, Role, StyledText, Window, div, prelude::*, px};
use std::rc::Rc;
use std::sync::Arc;

pub(crate) type OpenDiff = Rc<dyn Fn(&Arc<DiffDocument>, &mut Window, &mut App)>;

type ShowAll = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

pub(crate) struct ToolRenderer<'a> {
    review: &'a ReviewTheme,
    theme: UiTheme,
    highlighter: &'a mut SyntaxHighlighter,
    patches: &'a mut DiffPreviewCache,
    open_diff: OpenDiff,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolFold {
    Folded,
    Expanded,
    Full,
}

pub(crate) fn fold_header(
    kind: GroupKind,
    row: usize,
    items: &[ConversationItem],
    expanded: bool,
    theme: &UiTheme,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let colors = theme.colors;
    let work = Work::of(items);
    let failed = work.failed > 0;
    if kind == GroupKind::Run {
        let status = if failed { ToolStatus::Failed } else { ToolStatus::Success };
        let summary = work.details().collect::<Vec<_>>().join(" · ");
        return Disclosure::new(("tool-group", row), expanded, *theme)
            .h(rem(ROW_HEIGHT))
            .overflow_hidden()
            .on_toggle(on_toggle)
            .children(tool_line(
                status_icon(status, &format!("group-{row}"), theme),
                div().flex_none().child(format!("Ran {} tools", work.tools)),
                Some(summary.into_any_element()),
                failed,
                theme,
            ))
            .into_any_element();
    }
    let done = match work.tools {
        0 => "Thought".to_owned(),
        1 => "Ran 1 tool".to_owned(),
        tools => format!("Ran {tools} tools"),
    };
    let label = std::iter::once(done).chain(work.details()).collect::<Vec<_>>().join(" · ");
    let chevron = if expanded { IconName::ChevronDown } else { IconName::ChevronRight };
    let hairline = || div().flex_1().h(px(1.0)).bg(colors.border_subtle);
    let selector = format!("turn-summary-{row}");
    div()
        .id(("turn-summary", row))
        .debug_selector(move || selector)
        .role(Role::Button)
        .aria_expanded(expanded)
        .focusable()
        .tab_stop(true)
        .h(rem(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .typography(Typography::Body)
        .text_color(if failed { colors.destructive } else { colors.text_muted })
        .hover(move |summary| summary.text_color(colors.text_secondary))
        .focus_visible(move |summary| summary.text_color(colors.text))
        .on_click(on_toggle)
        .child(hairline())
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap_1()
                .child(label)
                .child(Icon::new(chevron, colors.text_muted).size(ICON_SIZE_SMALL)),
        )
        .child(hairline())
        .into_any_element()
}

impl<'a> ToolRenderer<'a> {
    pub(crate) fn new(
        theme: &'a DesktopTheme,
        highlighter: &'a mut SyntaxHighlighter,
        patches: &'a mut DiffPreviewCache,
        open_diff: OpenDiff,
    ) -> Self {
        Self { review: theme.review(), theme: theme.ui(), highlighter, patches, open_diff }
    }

    pub(crate) fn render(
        &mut self,
        tool: &ToolCall,
        index: usize,
        fold: ToolFold,
        on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
        on_show_all: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> AnyElement {
        let theme = self.theme;
        let expanded = fold != ToolFold::Folded;
        let on_show_all: ShowAll = Rc::new(on_show_all);
        let diffs: Vec<_> = tool.diffs().collect();
        let title = div().debug_selector(move || format!("tool-title-{index}")).min_w_0().truncate().child(title(tool));
        let header = Disclosure::new(("tool-card", index), expanded, theme)
            .h(rem(ROW_HEIGHT))
            .overflow_hidden()
            .on_toggle(on_toggle)
            .children(tool_line(
                status_icon(tool.status, tool.id(), &theme),
                title,
                self.summary(tool),
                tool.status == ToolStatus::Failed,
                &theme,
            ))
            .when(!diffs.is_empty(), |header| header.child(self.change_summary(tool, &diffs, index)));
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
                        .border_color(theme.colors.border_subtle)
                        .when(expanded, |body| {
                            body.child(self.details(tool, index, fold == ToolFold::Full, &on_show_all))
                        })
                        .when(expanded && !diffs.is_empty(), |body| body.child(self.changes(&diffs, index)))
                        .when(!tool.sub_agents.is_empty(), |body| body.child(self.sub_agent_tree(&tool.sub_agents))),
                )
            })
            .into_any_element()
    }

    fn summary(&mut self, tool: &ToolCall) -> Option<AnyElement> {
        if tool.status == ToolStatus::Failed
            && let Some(error) = output_headline(tool)
        {
            return Some(error.to_owned().into_any_element());
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
            let command = StyledText::new(format!("$ {first}")).with_highlights(highlights);
            return Some(div().font_family(CODE_FONT).child(command).into_any_element());
        }
        if let Some(value) = tool.display_value().and_then(first_line) {
            return Some(value.to_owned().into_any_element());
        }
        let path = tool.changed_paths().first()?.display().to_string();
        Some(div().font_family(CODE_FONT).child(path).into_any_element())
    }

    fn details(&mut self, tool: &ToolCall, index: usize, full: bool, on_show_all: &ShowAll) -> Div {
        let theme = self.theme;
        let command = tool.bash_command().map(|command| {
            let highlights = highlight(command, LanguageHint::Id("bash"), &self.review.syntax, self.highlighter);
            section("Command", StyledText::new(command.to_owned()).with_highlights(highlights), &theme)
        });
        let input = if tool.diffs().next().is_some() { String::new() } else { tool.raw_input() };
        let output = output_text(tool);
        let clipped = |label: &'static str, text: &str| {
            let (shown, hidden) = if full { (text, 0) } else { clip(text) };
            let id = format!("tool-{}-more-{index}", label.to_lowercase());
            section(label, shown.to_owned(), &theme)
                .children((hidden > 0).then(|| show_all(id, hidden, &theme, on_show_all.clone())))
        };
        div()
            .debug_selector(move || format!("tool-details-{index}"))
            .flex()
            .flex_col()
            .gap_3()
            .children(command)
            .when(!input.is_empty(), |details| details.child(clipped("Input", &input)))
            .when(!output.is_empty(), |details| details.child(clipped("Output", &output)))
    }

    fn changes(&mut self, diffs: &[&acp::Diff], index: usize) -> Div {
        let theme = self.theme;
        let mut changes = div()
            .debug_selector(move || format!("tool-changes-{index}"))
            .flex()
            .flex_col()
            .gap_2()
            .font_family(CODE_FONT)
            .typography(Typography::Small);
        for (position, diff) in diffs.iter().enumerate() {
            changes = match diff.patch.as_ref().filter(|patch| !patch.text.trim().is_empty()) {
                Some(patch) if patch.format == acp::DiffPatchFormat::GitPatch => {
                    let id = ElementId::named_usize(format!("tool-diff-{index}"), position);
                    let open = self.open_diff.clone();
                    let open = move |document: &Arc<DiffDocument>, window: &mut Window, cx: &mut App| {
                        open(document, window, cx);
                    };
                    changes.child(self.patches.render(id, &patch.text, self.review, self.highlighter, open))
                }
                Some(patch) => changes.child(section("Patch", patch.text.clone(), &theme)),
                None => changes.children(diff.changes.iter().map(|change| {
                    div().text_color(theme.colors.text_secondary).child(diff_change_label(change)).into_any_element()
                })),
            };
        }
        changes
    }

    fn change_summary(&mut self, tool: &ToolCall, diffs: &[&acp::Diff], index: usize) -> impl IntoElement + use<> {
        let (added, removed) = diffs
            .iter()
            .filter_map(|diff| diff.patch.as_ref())
            .filter_map(|patch| self.patches.stats(&patch.text))
            .fold((0, 0), |(added, removed), (more, fewer)| (added + more, removed + fewer));
        let files = tool.changed_paths().len();
        let changed_lines = added > 0 || removed > 0;
        div()
            .debug_selector(move || format!("tool-change-summary-{index}"))
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .typography(Typography::Caption)
            .text_color(self.theme.colors.text_muted)
            .when(files > 1 || !changed_lines, |summary| {
                summary.child(match files {
                    0 => "File changes".to_owned(),
                    1 => "1 file".to_owned(),
                    _ => format!("{files} files"),
                })
            })
            .when(changed_lines, |summary| {
                summary.children(stat_badges(&self.review.diff, LineStat { added, deleted: removed, files }))
            })
    }

    fn sub_agent_tree(&mut self, sub_agents: &[SubAgent]) -> Div {
        let theme = self.theme;
        let mut tree = div().flex().flex_col().gap_1().typography(Typography::Small);
        for agent in sub_agents {
            let agent_id = agent.session_id().0.to_string();
            let icon = agent.status().map_or_else(
                || Icon::new(IconName::Circle, theme.colors.text_muted).size(ICON_SIZE_SMALL).into_any_element(),
                |status| status_icon(status, &agent_id, &theme),
            );
            tree =
                tree.child(
                    div().flex().items_center().gap_2().child(icon).child(
                        div().typography(Typography::Label).child(agent.title().unwrap_or("sub-agent").to_owned()),
                    ),
                );
            let tool_calls: Vec<&ToolCall> = agent.tool_calls().collect();
            let hidden = tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOLS);
            if hidden > 0 {
                tree = tree.child(
                    div().pl_6().text_color(theme.colors.text_muted).child(format!("{hidden} earlier tool calls")),
                );
            }
            for tool in &tool_calls[hidden..] {
                let icon = status_icon(tool.status, &format!("{agent_id}-{}", tool.id()), &theme);
                let failed = tool.status == ToolStatus::Failed;
                tree = tree.child(div().pl_6().flex().items_center().gap_2().min_w_0().children(tool_line(
                    icon,
                    div().flex_none().child(title(tool)),
                    self.summary(tool),
                    failed,
                    &theme,
                )));
            }
        }
        tree
    }
}

const SUB_AGENT_VISIBLE_TOOLS: usize = 3;

const SECTION_MAX_LINES: usize = 200;

const SECTION_MAX_BYTES: usize = 16 * 1024;

fn tool_line(
    icon: AnyElement,
    title: Div,
    summary: Option<AnyElement>,
    failed: bool,
    theme: &UiTheme,
) -> [AnyElement; 3] {
    let colors = theme.colors;
    let title = title.typography(Typography::BodyStrong).text_color(colors.text_secondary);
    let summary = div()
        .flex_1()
        .min_w_0()
        .truncate()
        .typography(Typography::Body)
        .text_color(if failed { colors.destructive } else { colors.text_muted })
        .children(summary);
    [icon, title.into_any_element(), summary.into_any_element()]
}

fn output_text(tool: &ToolCall) -> String {
    tool.content().iter().filter_map(content_text).collect::<Vec<_>>().join("\n")
}

fn output_headline(tool: &ToolCall) -> Option<&str> {
    tool.content().iter().filter_map(content_text).find_map(first_line)
}

fn content_text(content: &acp::ToolCallContent) -> Option<&str> {
    match content {
        acp::ToolCallContent::Content(content) => match &content.content {
            acp::ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        },
        _ => None,
    }
}

fn title(tool: &ToolCall) -> String {
    tool.headline().unwrap_or("Tool call").to_owned()
}

fn section(label: &'static str, text: impl IntoElement, theme: &UiTheme) -> Div {
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

fn clip(text: &str) -> (&str, usize) {
    let lines = text.match_indices('\n').nth(SECTION_MAX_LINES - 1).map_or(text.len(), |(end, _)| end);
    let mut end = text.floor_char_boundary(lines.min(SECTION_MAX_BYTES));
    if end < lines
        && let Some(newline) = text[..end].rfind('\n')
    {
        end = newline;
    }
    let rest = &text[end..];
    (&text[..end], rest.strip_prefix('\n').unwrap_or(rest).lines().count())
}

fn show_all(id: String, hidden: usize, theme: &UiTheme, on_click: ShowAll) -> Button {
    let label = if hidden == 1 { "Show 1 more line".to_owned() } else { format!("Show {hidden} more lines") };
    Button::new(id, label, *theme)
        .variant(ButtonVariant::Ghost)
        .size(ControlSize::Small)
        .on_click(move |event, window, cx| on_click(event, window, cx))
}

fn status_icon(status: ToolStatus, id: &str, theme: &UiTheme) -> AnyElement {
    let (icon, color) = match status {
        ToolStatus::Running => {
            return Waveform::new(format!("tool-waveform-{id}"), theme.colors.info)
                .size(ICON_SIZE_SMALL)
                .into_any_element();
        }
        ToolStatus::Success => (IconName::Check, theme.colors.text_muted),
        ToolStatus::Failed => (IconName::CircleX, theme.colors.destructive),
        ToolStatus::Cancelled => (IconName::CircleSlash, theme.colors.text_muted),
    };
    Icon::new(icon, color).size(ICON_SIZE_SMALL).into_any_element()
}
