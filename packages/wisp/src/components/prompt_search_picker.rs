use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use std::ops::Range;
use std::path::{Path, PathBuf};
use tui::{
    Combobox, Component, Event, Frame, KeyCode, KeyModifiers, Line, Searchable, Style, Theme, ViewContext,
    display_width_text,
};
use unicode_width::UnicodeWidthChar;

/// Newtype wrapper to `impl Searchable` (foreign trait) for the foreign
/// `PromptSearchResult` type. The `Combobox` filtering path never runs — items
/// are pre-matched server-side and pushed via `Combobox::from_matches` — but
/// the trait bound is required for the rest of `Combobox`'s API.
#[derive(Clone)]
struct PromptResultItem(PromptSearchResult);

impl Searchable for PromptResultItem {
    fn search_text(&self) -> String {
        self.0.prompt.clone()
    }
}

/// Land the cursor at the end of the matched substring so the user can
/// immediately edit at the hit site instead of at end-of-prompt. Clamps to a
/// valid char boundary.
pub fn cursor_at_match_end(prompt: &str, match_end: usize) -> usize {
    let mut idx = match_end.min(prompt.len());
    while idx > 0 && !prompt.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

pub enum PromptSearchPickerMessage {
    Cancel,
    Confirm,
    QueryChanged(String),
    SelectionChanged,
}

/// A lightweight agent-backed prompt-history search picker.
///
/// Owns the in-progress query and a `Combobox` of results pushed in from the
/// backend via [`PromptSearchPicker::on_results`]. JSON-RPC correlates each
/// response with its request, so the picker only checks `query` equality when
/// deciding whether a response is stale.
pub struct PromptSearchPicker {
    query: String,
    combobox: Combobox<PromptResultItem>,
    loading: bool,
    error: Option<String>,
}

impl Default for PromptSearchPicker {
    fn default() -> Self {
        Self::new()
    }
}

impl PromptSearchPicker {
    pub fn new() -> Self {
        Self { query: String::new(), combobox: Combobox::from_matches(Vec::new()), loading: false, error: None }
    }

    pub fn selected_result(&self) -> Option<&PromptSearchResult> {
        self.combobox.selected().map(|item| &item.0)
    }

    /// Apply a response from the backend. Stale responses (different `query`)
    /// are ignored.
    pub fn on_results(&mut self, response: PromptSearchResponse) -> bool {
        if response.query != self.query {
            return false;
        }
        self.combobox = Combobox::from_matches(response.results.into_iter().map(PromptResultItem).collect());
        self.loading = false;
        self.error = None;
        true
    }

    /// Apply a failure from the backend. Stale failures are ignored.
    pub fn on_failed(&mut self, query: &str, error: String) -> bool {
        if self.query != query {
            return false;
        }
        self.combobox = Combobox::from_matches(Vec::new());
        self.loading = false;
        self.error = Some(error);
        true
    }

    fn refresh_query_state(&mut self) {
        self.error = None;
        if self.query.trim().is_empty() {
            self.combobox = Combobox::from_matches(Vec::new());
            self.loading = false;
        } else {
            self.loading = true;
        }
    }
}

impl Component for PromptSearchPicker {
    type Message = PromptSearchPickerMessage;

    async fn on_event(&mut self, event: &Event) -> Option<Vec<Self::Message>> {
        match event {
            Event::Paste(text) => {
                let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
                if sanitized.is_empty() {
                    return Some(vec![]);
                }
                self.query.push_str(&sanitized);
                self.refresh_query_state();
                Some(vec![PromptSearchPickerMessage::QueryChanged(self.query.clone())])
            }
            Event::Key(key_event) => match key_event.code {
                KeyCode::Esc => Some(vec![PromptSearchPickerMessage::Cancel]),
                KeyCode::Enter => Some(vec![if self.selected_result().is_some() {
                    PromptSearchPickerMessage::Confirm
                } else {
                    PromptSearchPickerMessage::Cancel
                }]),
                KeyCode::Down => {
                    self.combobox.move_down();
                    Some(vec![PromptSearchPickerMessage::SelectionChanged])
                }
                KeyCode::Up => {
                    self.combobox.move_up();
                    Some(vec![PromptSearchPickerMessage::SelectionChanged])
                }
                KeyCode::Backspace => {
                    if self.query.pop().is_some() {
                        self.refresh_query_state();
                        Some(vec![PromptSearchPickerMessage::QueryChanged(self.query.clone())])
                    } else {
                        Some(vec![])
                    }
                }
                KeyCode::Char(c) if !key_event.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    self.query.push(c);
                    self.refresh_query_state();
                    Some(vec![PromptSearchPickerMessage::QueryChanged(self.query.clone())])
                }
                _ => Some(vec![]),
            },
            _ => None,
        }
    }

    fn render(&mut self, context: &ViewContext) -> Frame {
        let mut lines = Vec::new();
        let muted = context.theme.muted();
        let info = context.theme.info();

        let header = format!("history search: {}", self.query);
        let mut header_line = Line::default();
        header_line.push_styled(header, info);
        lines.push(header_line);

        if let Some(err) = &self.error {
            lines.push(Line::styled(format!("  error: {err}"), muted));
            return Frame::new(lines);
        }

        if self.query.trim().is_empty() {
            lines.push(Line::styled("  type to search prompt history".to_string(), muted));
            return Frame::new(lines);
        }

        if self.loading && self.combobox.is_empty() {
            lines.push(Line::styled("  searching…".to_string(), muted));
            return Frame::new(lines);
        }

        if self.combobox.is_empty() {
            lines.push(Line::styled("  no matching prompts".to_string(), muted));
            return Frame::new(lines);
        }

        let item_lines =
            self.combobox.render_items(context, |item, is_selected, ctx| render_picker_row(&item.0, is_selected, ctx));
        lines.extend(item_lines);
        Frame::new(lines)
    }
}

/// Render one picker row: highlighted prompt on the left, muted cwd on the right.
fn render_picker_row(item: &PromptSearchResult, is_selected: bool, ctx: &ViewContext) -> Line {
    let max_width = usize::from(ctx.size.width).max(1);
    let theme = &ctx.theme;
    let styles = RowStyles::for_row(theme, is_selected);

    let cwd_display = abbreviate_cwd(&item.cwd, MAX_CWD_WIDTH);
    let cwd_width = display_width_text(&cwd_display);
    let (prompt_budget, show_cwd) = layout_widths(max_width, cwd_width);

    let mut line = Line::default();
    let prompt_width = push_prompt_with_highlight(
        &mut line,
        &item.prompt,
        item.match_start..item.match_end,
        prompt_budget,
        styles.base,
        styles.highlight,
    );

    if show_cwd {
        let pad = max_width.saturating_sub(prompt_width + cwd_width);
        if pad > 0 {
            line.push_with_style(" ".repeat(pad), styles.base);
        }
        line.push_with_style(cwd_display, styles.muted);
    }

    if is_selected { line.with_fill(theme.highlight_bg()) } else { line }
}

const MAX_CWD_WIDTH: usize = 32;
const CWD_GAP: usize = 2;
const MIN_PROMPT_WIDTH: usize = 16;
const ELLIPSIS: &str = "...";
const ELLIPSIS_WIDTH: usize = 3;

struct RowStyles {
    base: Style,
    highlight: Style,
    muted: Style,
}

impl RowStyles {
    fn for_row(theme: &Theme, is_selected: bool) -> Self {
        if is_selected {
            Self {
                base: theme.selected_row_style(),
                highlight: theme.selected_row_style_with_fg(theme.warning()),
                muted: theme.selected_row_style_with_fg(theme.muted()),
            }
        } else {
            Self {
                base: Style::fg(theme.text_primary()),
                highlight: Style::fg(theme.warning()),
                muted: Style::fg(theme.muted()),
            }
        }
    }
}

fn layout_widths(max_width: usize, cwd_width: usize) -> (usize, bool) {
    if cwd_width > 0 && max_width >= cwd_width + CWD_GAP + MIN_PROMPT_WIDTH {
        (max_width - cwd_width - CWD_GAP, true)
    } else {
        (max_width, false)
    }
}

/// Whitespace-collapses `prompt`, truncates to fit `max_width` display columns
/// (appending an ellipsis if it overflows), and pushes styled spans onto `line`
/// — coloring chars whose byte offset falls within `highlight` with
/// `highlight_style`, the rest with `base_style`.
///
/// Returns the display width actually pushed.
fn push_prompt_with_highlight(
    line: &mut Line,
    prompt: &str,
    highlight: Range<usize>,
    max_width: usize,
    base_style: Style,
    highlight_style: Style,
) -> usize {
    if max_width == 0 {
        return 0;
    }
    let use_ellipsis = max_width >= ELLIPSIS_WIDTH;
    let budget = if use_ellipsis { max_width - ELLIPSIS_WIDTH } else { max_width };

    let mut visible: Vec<(char, bool)> = Vec::new();
    let mut visible_width = 0usize; // width of chars currently in `visible`
    let mut budget_width = 0usize; // width up to `fit_end`
    let mut fit_end = 0usize; // number of chars in `visible` that fit within `budget`
    let mut last_was_ws = false;
    let mut overflowed = false;

    for (i, ch) in prompt.char_indices() {
        let in_hl = i >= highlight.start && i < highlight.end;
        let out_ch = if ch.is_whitespace() {
            if last_was_ws {
                continue;
            }
            last_was_ws = true;
            ' '
        } else {
            last_was_ws = false;
            ch
        };

        let cw = UnicodeWidthChar::width(out_ch).unwrap_or(0);
        if visible_width + cw > max_width {
            overflowed = true;
            break;
        }
        visible_width += cw;
        visible.push((out_ch, in_hl));
        if visible_width <= budget {
            fit_end = visible.len();
            budget_width = visible_width;
        }
    }

    let (kept, kept_width) = if overflowed && use_ellipsis {
        visible.truncate(fit_end);
        (&visible[..], budget_width)
    } else {
        (&visible[..], visible_width)
    };

    let mut i = 0;
    while i < kept.len() {
        let in_hl = kept[i].1;
        let mut j = i + 1;
        while j < kept.len() && kept[j].1 == in_hl {
            j += 1;
        }
        let run: String = kept[i..j].iter().map(|(c, _)| *c).collect();
        line.push_with_style(run, if in_hl { highlight_style } else { base_style });
        i = j;
    }

    if overflowed && use_ellipsis {
        line.push_with_style(ELLIPSIS, base_style);
        kept_width + ELLIPSIS_WIDTH
    } else {
        kept_width
    }
}

/// Render `cwd` as a home-relative path, falling back to a basename when the
/// home-relative form exceeds `max_width`.
fn abbreviate_cwd(cwd: &Path, max_width: usize) -> String {
    let full = home_relative_path(cwd);
    if display_width_text(&full) <= max_width {
        return full;
    }
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| display_width_text(name) <= max_width)
        .unwrap_or(full)
}

fn home_relative_path(path: &Path) -> String {
    let Some(home) = home_dir() else {
        return path.display().to_string();
    };
    if path == home {
        return "~".to_string();
    }
    path.strip_prefix(&home)
        .ok()
        .filter(|relative| !relative.as_os_str().is_empty())
        .map_or_else(|| path.display().to_string(), |relative| format!("~/{}", relative.display()))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from)
}
