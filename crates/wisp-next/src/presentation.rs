use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::app::{HistoryItem, HistoryKind, SubAgentHistoryItem};
use crate::diff::render_diff;
use crate::markdown::render_markdown;
use crate::progress_indicator::spinner_frame;
use crate::settings::{ResolvedStatusLineSettings, UiSettings, resolve_status_line_settings};
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::tool_calls::{SUB_AGENT_VISIBLE_TOOL_LIMIT, ToolStatus};
use crate::wrap::{truncate_to_width, wrap_line, wrap_text};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

/// Turns transcript items into styled lines, and owns the scrollback those
/// lines accumulate into.
pub struct TranscriptRenderer {
    theme: Theme,
    highlighter: SyntaxHighlighter,
    committed_lines: Vec<Line<'static>>,
    transcript_generation: u64,
    theme_generation: u64,
    /// Markdown for the streaming tail, keyed by content hash and width, so a
    /// growing message is not re-parsed from scratch every frame.
    pending_markdown_cache: HashMap<(u64, u16), Vec<Line<'static>>>,
    settings: ResolvedStatusLineSettings,
}

impl TranscriptRenderer {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            theme: Theme::load(settings),
            highlighter: SyntaxHighlighter::new(),
            committed_lines: Vec::new(),
            transcript_generation: 0,
            theme_generation: 0,
            pending_markdown_cache: HashMap::new(),
            settings: resolve_status_line_settings(settings),
        }
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn settings(&self) -> &ResolvedStatusLineSettings {
        &self.settings
    }

    pub fn highlighter(&mut self) -> &mut SyntaxHighlighter {
        &mut self.highlighter
    }

    pub fn theme_generation(&self) -> u64 {
        self.theme_generation
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.theme_generation = self.theme_generation.wrapping_add(1);
        self.highlighter.clear();
        self.pending_markdown_cache.clear();
    }

    /// Renders items that have left the live viewport for good.
    pub fn history_lines(
        &mut self,
        items: &[HistoryItem],
        previous_kind: Option<HistoryKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        self.render_items(items, previous_kind, width, padding, spinner_tick, false)
    }

    /// Renders the still-mutating tail, caching markdown between frames.
    pub(crate) fn pending_history_lines(
        &mut self,
        items: &[HistoryItem],
        previous_kind: Option<HistoryKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        self.render_items(items, previous_kind, width, padding, spinner_tick, true)
    }

    pub(crate) fn sync_transcript_generation(&mut self, generation: u64) {
        if self.transcript_generation != generation {
            self.committed_lines.clear();
            self.pending_markdown_cache.clear();
            self.transcript_generation = generation;
        }
    }

    pub(crate) fn append_committed_lines(&mut self, lines: Vec<Line<'static>>) {
        self.committed_lines.extend(lines);
    }

    pub(crate) fn committed_lines(&self) -> &[Line<'static>] {
        &self.committed_lines
    }

    pub(crate) fn take_committed_overflow(&mut self, visible_lines: usize) -> Vec<Line<'static>> {
        let overflow = self.committed_lines.len().saturating_sub(visible_lines);
        self.committed_lines.drain(..overflow).collect()
    }

    /// Renders `items` back to back, inserting a blank line wherever the kind
    /// of content changes so speakers and tool calls stay visually separated.
    fn render_items(
        &mut self,
        items: &[HistoryItem],
        previous_kind: Option<HistoryKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
        cache_markdown: bool,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut previous = previous_kind;
        for item in items {
            let kind = item.kind();
            if previous.is_some_and(|value| value != kind) {
                lines.push(Line::default());
            }
            match item {
                HistoryItem::Text(text) if cache_markdown => {
                    lines.extend(self.cached_markdown(text, width, padding));
                }
                _ => lines.extend(self.item_lines(item, width, padding, spinner_tick)),
            }
            previous = Some(kind);
        }
        lines
    }

    fn cached_markdown(&mut self, text: &str, width: u16, padding: usize) -> Vec<Line<'static>> {
        let content_width = content_width(width, padding);
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let key = (hasher.finish(), content_width);
        let theme = self.theme.clone();
        self.pending_markdown_cache
            .entry(key)
            .or_insert_with(|| {
                indent_lines(render_markdown(text, content_width, &theme, &mut self.highlighter), padding)
            })
            .clone()
    }

    fn item_lines(
        &mut self,
        item: &HistoryItem,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let content_width = content_width(width, padding);
        let theme = self.theme.clone();
        match item {
            HistoryItem::User(text) => user_block_lines(text, width, padding, &theme),
            HistoryItem::Text(text) => {
                indent_lines(render_markdown(text, content_width, &theme, &mut self.highlighter), padding)
            }
            HistoryItem::Thought(text) => indent_lines(
                wrap_line(
                    Line::styled(
                        text.clone(),
                        Style::new().fg(theme.blockquote).add_modifier(Modifier::ITALIC | Modifier::DIM),
                    ),
                    content_width,
                ),
                padding,
            ),
            HistoryItem::Tool { title, status, diff, raw_input, display_value, sub_agents } => {
                let mut lines =
                    vec![tool_line(title, raw_input, display_value.as_deref(), status, spinner_tick, padding, &theme)];
                if matches!(status, ToolStatus::Success)
                    && let Some(preview) = diff
                {
                    let rendered = render_diff(preview, content_width, &theme, &mut self.highlighter);
                    lines.extend(indent_lines(rendered, padding));
                }
                if !sub_agents.is_empty() {
                    lines.push(Line::default());
                    lines.extend(sub_agent_tree_lines(sub_agents, spinner_tick, padding, &theme));
                }
                lines
            }
        }
    }
}

pub(crate) fn indent_lines(lines: Vec<Line<'static>>, padding: usize) -> Vec<Line<'static>> {
    let prefix = " ".repeat(padding);
    lines
        .into_iter()
        .map(|mut line| {
            line.spans.insert(0, Span::raw(prefix.clone()));
            line
        })
        .collect()
}

fn content_width(width: u16, padding: usize) -> u16 {
    width.saturating_sub(u16::try_from(padding.saturating_mul(2)).unwrap_or(u16::MAX)).max(1)
}

/// The user's own message, drawn as a full-width tinted block.
fn user_block_lines(text: &str, width: u16, padding: usize, theme: &Theme) -> Vec<Line<'static>> {
    let block_style = Style::new().bg(theme.sidebar_bg).fg(theme.text_primary);
    let width = usize::from(width);
    let content_width = width.saturating_sub(padding.saturating_mul(2)).max(1);
    let blank = Line::styled(" ".repeat(width), block_style);
    let mut lines = vec![blank.clone()];
    for wrapped in wrap_text(text.trim_end_matches('\n'), content_width) {
        let fill = width.saturating_sub(padding + wrapped.width());
        lines.push(Line::styled(format!("{}{wrapped}{}", " ".repeat(padding), " ".repeat(fill)), block_style));
    }
    lines.push(blank);
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

const MAX_TOOL_ARG_WIDTH: usize = 200;

fn tool_line(
    title: &str,
    raw_input: &str,
    display_value: Option<&str>,
    status: &ToolStatus,
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![
        Span::raw(" ".repeat(padding)),
        status_glyph(status, spinner_tick, theme),
        Span::raw(" "),
        Span::styled(title.to_string(), Style::new().fg(theme.text_primary)),
        Span::styled(tool_detail(display_value, raw_input, status), Style::new().fg(theme.muted)),
    ];
    if let ToolStatus::Error(cause) = status {
        spans.push(Span::styled(format!(" {cause}"), Style::new().fg(theme.error)));
    }
    Line::from(spans)
}

/// Tree of sub-agents beneath a spawning tool, each with its recent tool calls.
fn sub_agent_tree_lines(
    sub_agents: &[SubAgentHistoryItem],
    spinner_tick: usize,
    padding: usize,
    theme: &Theme,
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
        let hidden = agent.tools.len().saturating_sub(SUB_AGENT_VISIBLE_TOOL_LIMIT);
        if hidden > 0 {
            lines.push(Line::styled(format!("{pad}  … {hidden} earlier tool calls"), muted));
        }

        let visible: Vec<_> = agent.tools.iter().skip(hidden).collect();
        for (index, tool) in visible.iter().enumerate() {
            let branch = if index + 1 == visible.len() { "  └─ " } else { "  ├─ " };
            let mut line = Line::from(vec![
                Span::raw(format!("{pad}{branch}")),
                status_glyph(&tool.status, spinner_tick, theme),
                Span::raw(format!(" {}", tool.name)),
                Span::styled(tool_detail(tool.display_value.as_deref(), &tool.arguments, &tool.status), muted),
            ]);
            if let ToolStatus::Error(message) = &tool.status {
                line.push_span(Span::styled(format!(" {message}"), Style::new().fg(theme.error)));
            }
            lines.push(line);
        }
    }

    lines
}
