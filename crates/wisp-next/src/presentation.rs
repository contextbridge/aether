use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::app::{HistoryItem, HistoryKind};
use crate::diff::render_diff;
use crate::generation::Generation;
use crate::markdown::render_markdown;
use crate::progress_indicator::spinner_frame;
use crate::render_context::RenderContext;
use crate::settings::UiSettings;
use crate::syntax::SyntaxHighlighter;
use crate::theme::Theme;
use crate::tool_calls::{SUB_AGENT_VISIBLE_TOOL_LIMIT, SubAgentState, ToolStatus};
use crate::wrap::{truncate_to_width, wrap_line, wrap_text};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

const MAX_TOOL_ARG_WIDTH: usize = 200;

/// Which half of the transcript is being rendered.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Segment {
    /// Has left the live viewport for good.
    Committed,
    /// Still mutating, so its markdown is cached between frames.
    Live,
}

/// Turns transcript items into styled lines, and owns the two things that takes:
/// the theme and the syntax highlighter.
///
/// Methods destructure `self` rather than taking `&self.theme` through `&mut
/// self`, so the theme is borrowed alongside the highlighter instead of cloned
/// once per item per frame.
pub struct Presenter {
    theme: Theme,
    highlighter: SyntaxHighlighter,
    theme_generation: Generation,
    pending_markdown_cache: PendingMarkdownCache,
    scrollback: Scrollback,
}

/// The rendered lines that have left the live viewport. What still fits on
/// screen is drawn; anything older is handed to the terminal's own history.
#[derive(Default)]
pub struct Scrollback {
    lines: Vec<Line<'static>>,
    generation: Generation,
}

impl Scrollback {
    pub fn lines(&self) -> &[Line<'static>] {
        &self.lines
    }

    pub fn append(&mut self, lines: Vec<Line<'static>>) {
        self.lines.extend(lines);
    }

    /// Everything beyond the newest `visible` rows, for handing to the terminal.
    pub fn take_overflow(&mut self, visible: usize) -> Vec<Line<'static>> {
        let overflow = self.lines.len().saturating_sub(visible);
        self.lines.drain(..overflow).collect()
    }

    /// Drops the scrollback when the conversation it belongs to is swapped out,
    /// reporting whether that happened.
    fn sync(&mut self, generation: Generation) -> bool {
        if self.generation == generation {
            return false;
        }
        self.lines.clear();
        self.generation = generation;
        true
    }
}

impl Presenter {
    pub fn new(settings: &UiSettings) -> Self {
        Self {
            theme: Theme::load(settings),
            highlighter: SyntaxHighlighter::new(),
            theme_generation: Generation::default(),
            pending_markdown_cache: PendingMarkdownCache::default(),
            scrollback: Scrollback::default(),
        }
    }

    pub fn scrollback(&self) -> &Scrollback {
        &self.scrollback
    }

    pub fn scrollback_mut(&mut self) -> &mut Scrollback {
        &mut self.scrollback
    }

    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.theme_generation.bump();
        self.highlighter.clear();
        self.pending_markdown_cache.clear();
    }

    /// The rendering services a full-screen surface or overlay draws with.
    pub fn context(&mut self) -> RenderContext<'_> {
        RenderContext {
            theme: &self.theme,
            highlighter: &mut self.highlighter,
            theme_generation: self.theme_generation,
        }
    }

    /// Drops everything cached for a conversation that has been swapped out.
    pub fn sync_transcript_generation(&mut self, generation: Generation) {
        if self.scrollback.sync(generation) {
            self.pending_markdown_cache.clear();
        }
    }

    /// Renders `items` back to back, inserting a blank line wherever the kind
    /// of content changes so speakers and tool calls stay visually separated.
    pub fn lines(
        &mut self,
        segment: Segment,
        items: &[HistoryItem<'_>],
        previous_kind: Option<HistoryKind>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        let mut previous = previous_kind;
        for item in items {
            let kind = item.kind();
            if previous.is_some_and(|value| value != kind) {
                lines.push(Line::default());
            }
            match item {
                // Only the live tail is worth caching: a committed segment is
                // rendered once and never asked for again.
                HistoryItem::Text(text) if segment == Segment::Live => {
                    lines.extend(self.cached_markdown(text, width, padding).iter().cloned());
                }
                _ => lines.extend(self.item_lines(item, width, padding, spinner_tick)),
            }
            previous = Some(kind);
        }
        if segment == Segment::Live {
            self.pending_markdown_cache.end_frame();
        }
        lines
    }

    fn cached_markdown(&mut self, text: &str, width: u16, padding: usize) -> Rc<[Line<'static>]> {
        let Self { theme, highlighter, pending_markdown_cache, .. } = self;
        let content_width = content_width(width, padding);
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        pending_markdown_cache.get_or_insert_with((hasher.finish(), content_width), || {
            indent_lines(render_markdown(text, content_width, theme, highlighter), padding)
        })
    }

    fn item_lines(
        &mut self,
        item: &HistoryItem<'_>,
        width: u16,
        padding: usize,
        spinner_tick: usize,
    ) -> Vec<Line<'static>> {
        let Self { theme, highlighter, .. } = self;
        let content_width = content_width(width, padding);
        match item {
            HistoryItem::User(text) => user_block_lines(text, width, padding, theme),
            HistoryItem::Text(text) => indent_lines(render_markdown(text, content_width, theme, highlighter), padding),
            HistoryItem::Thought(text) => indent_lines(
                wrap_line(
                    Line::styled(
                        text.to_string(),
                        Style::new().fg(theme.blockquote).add_modifier(Modifier::ITALIC | Modifier::DIM),
                    ),
                    content_width,
                ),
                padding,
            ),
            HistoryItem::Tool { title, status, diff, raw_input, display_value, sub_agents } => {
                let mut lines =
                    vec![tool_line(title, raw_input, display_value.as_deref(), status, spinner_tick, padding, theme)];
                if matches!(status.as_ref(), ToolStatus::Success)
                    && let Some(preview) = diff
                {
                    let rendered = render_diff(preview, content_width, theme, highlighter);
                    lines.extend(indent_lines(rendered, padding));
                }
                if !sub_agents.is_empty() {
                    lines.push(Line::default());
                    lines.extend(sub_agent_tree_lines(sub_agents, spinner_tick, padding, theme));
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

/// Rendered markdown for the streaming tail, keyed by content hash and width.
///
/// Each frame's entries are collected separately and become the whole cache
/// when the frame ends, so a message that grows by one chunk per frame reuses
/// its previous render without accumulating an entry per intermediate prefix.
#[derive(Default)]
struct PendingMarkdownCache {
    current: HashMap<(u64, u16), Rc<[Line<'static>]>>,
    frame: HashMap<(u64, u16), Rc<[Line<'static>]>>,
}

impl PendingMarkdownCache {
    fn get_or_insert_with(
        &mut self,
        key: (u64, u16),
        build: impl FnOnce() -> Vec<Line<'static>>,
    ) -> Rc<[Line<'static>]> {
        let lines = self.frame.remove(&key).or_else(|| self.current.remove(&key)).unwrap_or_else(|| Rc::from(build()));
        self.frame.insert(key, Rc::clone(&lines));
        lines
    }

    /// Drops every entry the frame that just rendered did not use.
    fn end_frame(&mut self) {
        self.current = std::mem::take(&mut self.frame);
    }

    fn clear(&mut self) {
        self.current.clear();
        self.frame.clear();
    }
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
    sub_agents: &[SubAgentState],
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
        let hidden = agent.tool_calls.len().saturating_sub(SUB_AGENT_VISIBLE_TOOL_LIMIT);
        if hidden > 0 {
            lines.push(Line::styled(format!("{pad}  … {hidden} earlier tool calls"), muted));
        }

        let visible: Vec<_> = agent.tool_calls.iter().skip(hidden).collect();
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

#[cfg(test)]
mod tests {
    use super::{PendingMarkdownCache, Rc};
    use ratatui::text::Line;

    fn render(cache: &mut PendingMarkdownCache, key: u64, text: &str) -> Rc<[Line<'static>]> {
        cache.get_or_insert_with((key, 80), || vec![Line::raw(text.to_string())])
    }

    #[test]
    fn holds_only_what_the_last_frame_rendered() {
        let mut cache = PendingMarkdownCache::default();
        // A streaming message hashes differently after every chunk, so an
        // unpruned cache would keep one render per intermediate prefix.
        for (key, text) in [(1, "a"), (2, "ab"), (3, "abc")] {
            render(&mut cache, key, text);
            cache.end_frame();
        }

        assert_eq!(cache.current.len(), 1);
    }

    #[test]
    fn reuses_the_previous_frames_render() {
        let mut cache = PendingMarkdownCache::default();
        render(&mut cache, 1, "built once");
        cache.end_frame();

        let reused = cache.get_or_insert_with((1, 80), || panic!("cached render must not be rebuilt"));

        assert_eq!(*reused, [Line::raw("built once")]);
    }

    #[test]
    fn keeps_every_segment_a_single_frame_rendered() {
        let mut cache = PendingMarkdownCache::default();
        render(&mut cache, 1, "first");
        render(&mut cache, 2, "second");
        cache.end_frame();

        assert_eq!(cache.current.len(), 2);
    }
}
