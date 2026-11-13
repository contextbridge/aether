use super::code::highlights;
use super::groups::{GroupKind, Work};
use crate::ui::tokens::{ICON_SIZE_SMALL, ROW_HEIGHT};
use crate::ui::typography::BODY_STRONG;
use crate::ui::{
    ButtonVariant, CODE_FONT, ControlSize, Hint, Icon, IconName, Theme, UiStyles, WispState, button, disclosure, wisp,
};
use acp_utils::content::first_line;
use acp_utils::conversation::{
    ConversationItem, ConversationItemId, Revision, SubAgent, ToolCall, ToolStatus, diff_change_label,
};
use agent_client_protocol::v2::schema as acp;
use clankerdiff_core::{DiffDocument, LineStat};
use clankerdiff_rouge::PatchPreview;
use clankerdiff_rouge::style::stat_badges;
use clankerdiff_ui_core::ReviewTheme;
use rouge::core::{AnyView, Cursor, Div, FocusHandle, SpanStyle, Styled, View, div, px, rich_text, text, view};
use rouge::reactive::{Callback, ReadSignal};
use rouge::syntax::{LanguageHint, SyntaxCell};
use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

#[derive(Clone, Copy)]
pub(crate) struct ToolActions {
    pub(crate) open_diff: Callback<(Arc<DiffDocument>,)>,
    pub(crate) open_location: Callback<(acp::ToolCallLocation,)>,
}

#[derive(Default)]
pub(crate) struct ToolPreviews(HashMap<ConversationItemId, Rc<ToolPreview>>);

pub(crate) struct ToolPreview {
    revision: Revision,
    command: SyntaxCell,
    patches: Vec<Option<PatchPreview>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolFold {
    Folded,
    Expanded,
    Full,
}

impl ToolPreviews {
    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn get(&mut self, item: &ConversationItem, tool: &ToolCall) -> Rc<ToolPreview> {
        let preview = self.0.entry(item.id()).or_insert_with(|| Rc::new(ToolPreview::new(item.revision(), tool)));
        if preview.revision != item.revision() {
            *preview = Rc::new(ToolPreview::new(item.revision(), tool));
        }
        preview.clone()
    }
}

impl ToolPreview {
    fn new(revision: Revision, tool: &ToolCall) -> Self {
        let patches = tool
            .diffs()
            .map(|diff| {
                diff.patch
                    .as_ref()
                    .filter(|patch| patch.format == acp::DiffPatchFormat::GitPatch && !patch.text.trim().is_empty())
                    .map(|patch| PatchPreview::parse(&patch.text))
            })
            .collect();
        Self { revision, command: SyntaxCell::default(), patches }
    }

    fn patch(&self, position: usize) -> Option<&PatchPreview> {
        self.patches.get(position)?.as_ref()
    }
}

pub(crate) fn fold_header(
    kind: GroupKind,
    row: usize,
    items: &[ConversationItem],
    expanded: bool,
    on_toggle: Callback,
) -> AnyView {
    let theme = Theme::current();
    let work = Work::of(items);
    let failed = work.failed > 0;
    if kind == GroupKind::Run {
        let status = if failed { ToolStatus::Failed } else { ToolStatus::Success };
        let summary = work.details().collect::<Vec<_>>().join(" · ");
        let [icon, title, summary] = tool_line(
            status_icon(status, &format!("group-{row}")),
            view! { <div flex_none><text>{format!("Ran {} tools", work.tools)}</text></div> },
            Some(view! { <text no_wrap>{summary}</text> }.into_any()),
            failed,
        );
        return view! {
            <disclosure id={format!("tool-group-{row}")} expanded on_toggle h=ROW_HEIGHT overflow_hidden>
                <div flex items_center gap=8.0 min_w=0.0>{icon}{title}{summary}</div>
            </disclosure>
        }
        .into_any();
    }
    let done = match work.tools {
        0 => "Thought".to_owned(),
        1 => "Ran 1 tool".to_owned(),
        tools => format!("Ran {tools} tools"),
    };
    let label = std::iter::once(done).chain(work.details()).collect::<Vec<_>>().join(" · ");
    let chevron = if expanded { IconName::ChevronDown } else { IconName::ChevronRight };
    let hairline = || view! { <div flex_1 h=px(1.0) bg=theme.border_subtle/> };
    let color = if failed { theme.destructive } else { theme.text_muted };
    view! {
        <div test_id={format!("turn-summary-{row}")} h=ROW_HEIGHT flex items_center gap=12.0 body text_color=color
             on_press={move || on_toggle.call()}>
            {hairline()}
            <div flex_none flex items_center gap=4.0>
                <text>{label}</text>
                {Icon::new(chevron, theme.text_muted).size(ICON_SIZE_SMALL)}
            </div>
            {hairline()}
        </div>
    }
    .into_any()
}

pub(crate) struct ToolCard<'a> {
    pub(crate) review: &'a ReviewTheme,
    pub(crate) theme: ReadSignal<ReviewTheme>,
    pub(crate) preview: &'a ToolPreview,
    pub(crate) actions: ToolActions,
}

impl ToolCard<'_> {
    pub(crate) fn render(
        &self,
        tool: &ToolCall,
        index: usize,
        fold: ToolFold,
        focus: FocusHandle,
        on_toggle: Callback,
        on_show_all: Callback,
    ) -> AnyView {
        let theme = Theme::current();
        let expanded = fold != ToolFold::Folded;
        let diffs: Vec<&acp::Diff> = tool.diffs().collect();
        let title = view! {
            <div test_id={format!("tool-title-{index}")} min_w=0.0 overflow_hidden><text no_wrap>{title(tool)}</text></div>
        };
        let summary = self.summary(tool).map(|summary| match tool.locations().first() {
            Some(location) => self.location_link(location.clone(), index, summary),
            None => summary,
        });
        let [icon, title, summary] =
            tool_line(status_icon(tool.status, tool.id()), title, summary, tool.status == ToolStatus::Failed);
        let changes = (!diffs.is_empty()).then(|| self.change_summary(tool, index));
        let header = view! {
            <disclosure id={format!("tool-card-{index}")} expanded on_toggle h=ROW_HEIGHT overflow_hidden
                        track_focus=focus>
                <div flex items_center gap=8.0 min_w=0.0>{icon}{title}{summary}{changes}</div>
            </disclosure>
        };
        let body = (expanded || !tool.sub_agents.is_empty()).then(|| {
            let details = expanded.then(|| self.details(tool, index, fold == ToolFold::Full, on_show_all));
            let changes = (expanded && !diffs.is_empty()).then(|| self.changes(&diffs, index));
            let agents = (!tool.sub_agents.is_empty()).then(|| self.sub_agent_tree(&tool.sub_agents));
            view! {
                <div flex col gap=12.0 ml=8.0 mt=4.0 mb=8.0 pl=16.0 pr=8.0 py=4.0 border_l=px(1.0)
                     border_color=theme.border_subtle>
                    {details}
                    {changes}
                    {agents}
                </div>
            }
        });
        view! { <div flex col min_w=0.0>{header}{body}</div> }.into_any()
    }

    fn summary(&self, tool: &ToolCall) -> Option<AnyView> {
        if tool.status == ToolStatus::Failed
            && let Some(error) = output_headline(tool)
        {
            return Some(view! { <text no_wrap>{error.to_owned()}</text> }.into_any());
        }
        if let Some(command) = tool.bash_command()
            && let Some(first) = command.lines().next()
        {
            let spans = self
                .command_highlights(command)
                .into_iter()
                .filter_map(|(range, style)| {
                    (range.start < first.len()).then(|| (range.start + 2..range.end.min(first.len()) + 2, style))
                })
                .collect::<Vec<_>>();
            return Some(rich_text(format!("$ {first}"), spans).font(CODE_FONT).no_wrap().into_any());
        }
        if let Some(value) = tool.display_value().and_then(first_line) {
            return Some(view! { <text no_wrap>{value.to_owned()}</text> }.into_any());
        }
        let path = tool.changed_paths().first()?.display().to_string();
        Some(view! { <text code no_wrap>{path}</text> }.into_any())
    }

    fn command_highlights(&self, command: &str) -> Vec<(Range<usize>, SpanStyle)> {
        let syntax = self.preview.command.get(LanguageHint::Id("bash"), || command);
        highlights(command, &syntax.highlights, &self.review.syntax)
    }

    fn location_link(&self, location: acp::ToolCallLocation, index: usize, summary: AnyView) -> AnyView {
        let open = self.actions.open_location;
        let label = format!("Open {}", location.path.0.display());
        view! {
            <div test_id={format!("tool-location-{index}")} min_w=0.0 overflow_hidden cursor=Cursor::Pointer
                 on_click={move |event| {
                     event.stop_propagation();
                     open.call(location.clone());
                 }}>
                {summary}
            </div>
        }
        .tooltip(label)
        .into_any()
    }

    fn details(&self, tool: &ToolCall, index: usize, full: bool, on_show_all: Callback) -> Div {
        let command = tool.bash_command().map(|command| {
            let spans = self.command_highlights(command);
            section("Command", rich_text(command.to_owned(), spans).font(CODE_FONT).into_any())
        });
        let input = if tool.diffs().next().is_some() { String::new() } else { tool.raw_input() };
        let output = output_text(tool);
        let clipped = |label: &'static str, source: &str| {
            let (shown, hidden) = if full { (source, 0) } else { clip(source) };
            let more = (hidden > 0)
                .then(|| show_all(format!("tool-{}-more-{index}", label.to_lowercase()), hidden, on_show_all));
            section(label, view! { <text code>{shown.to_owned()}</text> }.into_any()).child(more)
        };
        let input = (!input.is_empty()).then(|| clipped("Input", &input));
        let output = (!output.is_empty()).then(|| clipped("Output", &output));
        view! {
            <div test_id={format!("tool-details-{index}")} flex col gap=12.0>{command}{input}{output}</div>
        }
    }

    fn changes(&self, diffs: &[&acp::Diff], index: usize) -> Div {
        let theme = Theme::current();
        let changes = diffs.iter().enumerate().map(|(position, diff)| {
            let patch = diff.patch.as_ref().filter(|patch| !patch.text.trim().is_empty());
            match (patch, self.preview.patch(position)) {
                (Some(_), Some(preview)) => {
                    let open = self.actions.open_diff;
                    preview
                        .render(
                            format!("tool-diff-{index}-{position}"),
                            self.review,
                            move |document: &Arc<DiffDocument>| {
                                open.call(document.clone());
                            },
                        )
                        .into_any()
                }
                (Some(patch), None) => {
                    section("Patch", view! { <text code>{patch.text.clone()}</text> }.into_any()).into_any()
                }
                (None, _) => {
                    let labels = diff.changes.iter().map(|change| {
                        view! { <text small color=theme.text_secondary>{diff_change_label(change)}</text> }
                    });
                    view! { <div flex col>{..labels}</div> }.into_any()
                }
            }
        });
        view! {
            <div test_id={format!("tool-changes-{index}")} flex col gap=8.0 font=CODE_FONT small>{..changes}</div>
        }
    }

    fn change_summary(&self, tool: &ToolCall, index: usize) -> AnyView {
        let (added, removed) = self
            .preview
            .patches
            .iter()
            .flatten()
            .filter_map(PatchPreview::stats)
            .fold((0, 0), |(added, removed), (more, fewer)| (added + more, removed + fewer));
        let files = tool.changed_paths().len();
        let changed_lines = added > 0 || removed > 0;
        let count = (files > 1 || !changed_lines).then(|| {
            let label = match files {
                0 => "File changes".to_owned(),
                1 => "1 file".to_owned(),
                _ => format!("{files} files"),
            };
            view! { <text caption muted no_wrap>{label}</text> }
        });
        let badges = changed_lines.then(|| stat_badges(LineStat { added, deleted: removed, files }, self.theme));
        view! {
            <div test_id={format!("tool-change-summary-{index}")} flex flex_none items_center gap=8.0 caption>
                {count}
                {badges}
            </div>
        }
        .into_any()
    }

    fn sub_agent_tree(&self, sub_agents: &[SubAgent]) -> Div {
        let theme = Theme::current();
        let mut rows: Vec<AnyView> = Vec::new();
        for agent in sub_agents {
            let agent_id = agent.session_id().0.to_string();
            let icon = agent.status().map_or_else(
                || Icon::new(IconName::Circle, theme.text_muted).size(ICON_SIZE_SMALL).into_any(),
                |status| status_icon(status, &agent_id),
            );
            let name = agent.title().unwrap_or("sub-agent").to_owned();
            rows.push(view! { <div flex items_center gap=8.0>{icon}<text label>{name}</text></div> }.into_any());
            let tool_calls: Vec<&ToolCall> = agent.tool_calls().collect();
            let hidden = tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOLS);
            if hidden > 0 {
                rows.push(
                    view! { <div pl=24.0><text muted>{format!("{hidden} earlier tool calls")}</text></div> }.into_any(),
                );
            }
            for tool in &tool_calls[hidden..] {
                let icon = status_icon(tool.status, &format!("{agent_id}-{}", tool.id()));
                let failed = tool.status == ToolStatus::Failed;
                let title = view! { <div flex_none><text>{title(tool)}</text></div> };
                let [icon, title, summary] = tool_line(icon, title, self.summary(tool), failed);
                rows.push(
                    view! { <div pl=24.0 flex items_center gap=8.0 min_w=0.0>{icon}{title}{summary}</div> }.into_any(),
                );
            }
        }
        view! { <div flex col gap=4.0 small>{..rows}</div> }
    }
}

const SUB_AGENT_VISIBLE_TOOLS: usize = 3;

const SECTION_MAX_LINES: usize = 200;

const SECTION_MAX_BYTES: usize = 16 * 1024;

fn tool_line(icon: AnyView, title: Div, summary: Option<AnyView>, failed: bool) -> [AnyView; 3] {
    let theme = Theme::current();
    let title = title.typography(BODY_STRONG).text_color(theme.text_secondary);
    let color = if failed { theme.destructive } else { theme.text_muted };
    let summary = view! { <div flex_1 min_w=0.0 overflow_hidden body text_color=color>{summary}</div> };
    [icon, title.into_any(), summary.into_any()]
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

fn section(label: &'static str, content: AnyView) -> Div {
    let theme = Theme::current();
    view! {
        <div flex col gap=4.0>
            <text label muted>{label}</text>
            <div text_color=theme.text_secondary font=CODE_FONT small>{content}</div>
        </div>
    }
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

fn show_all(id: String, hidden: usize, on_click: Callback) -> AnyView {
    let label = if hidden == 1 { "Show 1 more line".to_owned() } else { format!("Show {hidden} more lines") };
    view! { <button id label variant=ButtonVariant::Ghost size=ControlSize::Small on_click/> }
}

fn status_icon(status: ToolStatus, id: &str) -> AnyView {
    let theme = Theme::current();
    let (icon, color) = match status {
        ToolStatus::Running => {
            return view! {
                <div test_id={format!("tool-status-{id}")} flex_none flex items_center gap=4.0>
                    {wisp(ICON_SIZE_SMALL, theme.info, || WispState::Working).test_id(format!("tool-wisp-{id}"))}
                    <text small muted>"Running"</text>
                </div>
            }
            .into_any();
        }
        ToolStatus::Success => (IconName::Check, theme.text_muted),
        ToolStatus::Failed => (IconName::CircleX, theme.destructive),
        ToolStatus::Cancelled => (IconName::CircleSlash, theme.text_muted),
    };
    Icon::new(icon, color).size(ICON_SIZE_SMALL).into_any()
}
