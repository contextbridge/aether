use crate::theme::Theme;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use std::path::{Path, PathBuf};
use unicode_width::UnicodeWidthChar;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptSearchMessage {
    Cancel,
    Confirm,
    QueryChanged(String),
    SelectionChanged,
}

#[derive(Debug, Default)]
pub struct PromptSearchPicker {
    query: String,
    results: Vec<PromptSearchResult>,
    selected: usize,
    loading: bool,
    error: Option<String>,
    search_generation: u64,
}

impl PromptSearchPicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected_result(&self) -> Option<&PromptSearchResult> {
        self.results.get(self.selected)
    }

    pub fn search_generation(&self) -> u64 {
        self.search_generation
    }

    pub fn on_results(&mut self, response: PromptSearchResponse) -> bool {
        if response.query != self.query || response.search_generation != self.search_generation {
            return false;
        }
        self.results = response.results;
        self.selected = 0;
        self.loading = false;
        self.error = None;
        true
    }

    pub fn on_failed(&mut self, search_generation: u64, error: String) -> bool {
        if search_generation != self.search_generation {
            return false;
        }
        self.results.clear();
        self.selected = 0;
        self.loading = false;
        self.error = Some(error);
        true
    }

    fn refresh_query_state(&mut self) {
        self.error = None;
        if self.query.trim().is_empty() {
            self.results.clear();
            self.selected = 0;
            self.loading = false;
        } else {
            self.search_generation = self.search_generation.wrapping_add(1);
            self.loading = true;
        }
    }

    pub fn move_up(&mut self) {
        if !self.results.is_empty() {
            self.selected = self.selected.checked_sub(1).unwrap_or(self.results.len() - 1);
        }
    }

    pub fn move_down(&mut self) {
        if !self.results.is_empty() {
            self.selected = (self.selected + 1) % self.results.len();
        }
    }

    pub fn push_char(&mut self, c: char) -> PromptSearchMessage {
        self.query.push(c);
        self.refresh_query_state();
        PromptSearchMessage::QueryChanged(self.query.clone())
    }

    pub fn push_str(&mut self, text: &str) -> PromptSearchMessage {
        let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
        if sanitized.is_empty() {
            return PromptSearchMessage::QueryChanged(self.query.clone());
        }
        self.query.push_str(&sanitized);
        self.refresh_query_state();
        PromptSearchMessage::QueryChanged(self.query.clone())
    }

    pub fn backspace(&mut self) -> PromptSearchMessage {
        self.query.pop();
        self.refresh_query_state();
        PromptSearchMessage::QueryChanged(self.query.clone())
    }

    pub fn render(&self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let max_width = area.width.max(1) as usize;
        let mut y = area.y;

        let header = format!("history search: {}", self.query);
        let header_style = Style::new().fg(theme.info);
        render_line(buf, area.x, y, max_width, &header, header_style);
        y += 1;

        if let Some(err) = &self.error {
            let text = format!("  error: {err}");
            render_line(buf, area.x, y, max_width, &text, Style::new().fg(theme.muted));
            return;
        }

        if self.query.trim().is_empty() {
            render_line(buf, area.x, y, max_width, "  type to search prompt history", Style::new().fg(theme.muted));
            return;
        }

        if self.loading && self.results.is_empty() {
            render_line(buf, area.x, y, max_width, "  searching…", Style::new().fg(theme.muted));
            return;
        }

        if self.results.is_empty() {
            render_line(buf, area.x, y, max_width, "  no matching prompts", Style::new().fg(theme.muted));
            return;
        }

        let visible_rows = area.height.saturating_sub(1) as usize;
        for (row, result) in self.results.iter().take(visible_rows).enumerate() {
            let is_selected = row == self.selected;
            let row_style = if is_selected {
                Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)
            } else {
                Style::new().fg(theme.text_secondary)
            };
            let highlight_style = if is_selected {
                Style::new().fg(theme.warning).bg(theme.sidebar_bg)
            } else {
                Style::new().fg(theme.warning)
            };

            let cwd_display = abbreviate_cwd(&result.cwd, MAX_CWD_WIDTH);
            let cwd_width = display_width(&cwd_display);
            let prompt_budget = if cwd_width > 0 && max_width >= cwd_width + CWD_GAP + MIN_PROMPT_WIDTH {
                max_width - cwd_width - CWD_GAP
            } else {
                max_width
            };

            let mut spans: Vec<Span> = Vec::new();
            let prompt_width = push_prompt_with_highlight(
                &mut spans,
                &result.prompt,
                result.match_start..result.match_end,
                prompt_budget,
                row_style,
                highlight_style,
            );

            if prompt_budget < max_width {
                let cwd_start = prompt_width + CWD_GAP;
                let cwd_end = (cwd_start + cwd_width).min(max_width);
                if cwd_start < max_width {
                    for _ in prompt_width..cwd_start.min(max_width) {
                        spans.push(Span::styled(" ", row_style));
                    }
                    if cwd_end > cwd_start {
                        spans.push(Span::styled(
                            cwd_display,
                            Style::new().fg(theme.muted).bg(if is_selected { theme.sidebar_bg } else { Color::Reset }),
                        ));
                    }
                }
            }

            let line = Line::from(spans);
            buf.set_line(area.x, y, &line, u16::try_from(max_width).unwrap_or(u16::MAX));
            y += 1;
        }
    }
}

const MAX_CWD_WIDTH: usize = 32;
const CWD_GAP: usize = 2;
const MIN_PROMPT_WIDTH: usize = 16;
const ELLIPSIS: &str = "...";
const ELLIPSIS_WIDTH: usize = 3;

fn render_line(buf: &mut Buffer, x: u16, y: u16, max_width: usize, text: &str, style: Style) {
    let line = Line::styled(text, style);
    buf.set_line(x, y, &line, u16::try_from(max_width).unwrap_or(u16::MAX));
}

fn push_prompt_with_highlight(
    spans: &mut Vec<Span<'static>>,
    prompt: &str,
    highlight: std::ops::Range<usize>,
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
    let mut visible_width = 0usize;
    let mut budget_width = 0usize;
    let mut fit_end = 0usize;
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
        spans.push(Span::styled(run, if in_hl { highlight_style } else { base_style }));
        i = j;
    }

    if overflowed && use_ellipsis {
        spans.push(Span::styled(ELLIPSIS, base_style));
        kept_width + ELLIPSIS_WIDTH
    } else {
        kept_width
    }
}

fn display_width(text: &str) -> usize {
    unicode_width::UnicodeWidthStr::width(text)
}

fn abbreviate_cwd(cwd: &Path, max_width: usize) -> String {
    let full = home_relative_path(cwd);
    if display_width(&full) <= max_width {
        return full;
    }
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| display_width(name) <= max_width)
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

pub fn cursor_at_match_end(prompt: &str, match_end: usize) -> usize {
    let mut idx = match_end.min(prompt.len());
    while idx > 0 && !prompt.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
