use crate::session::workspace_status::home_relative_path;
use crate::view::edit_buffer::EditBuffer;
use crate::view::filterable_list::FilterableList;
use crate::theme::Theme;
use crate::view::wrap::truncate_spans;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};

use std::ops::Range;
use unicode_width::UnicodeWidthStr;

const MAX_CWD_WIDTH: usize = 32;
const CWD_GAP: usize = 2;
const MIN_PROMPT_WIDTH: usize = 16;

#[derive(Debug)]
pub struct PromptSearchPicker {
    query: EditBuffer,
    results: FilterableList<PromptSearchResult>,
    loading: bool,
    error: Option<String>,
}

impl PromptSearchPicker {
    pub fn new() -> Self {
        Self { query: EditBuffer::default(), results: Self::result_list(Vec::new()), loading: false, error: None }
    }

    pub fn query(&self) -> &str {
        self.query.text()
    }

    pub fn selected_result(&self) -> Option<&PromptSearchResult> {
        self.results.selected_entry()
    }

    pub fn results_mut(&mut self) -> &mut FilterableList<PromptSearchResult> {
        &mut self.results
    }

    /// Responses echo the query they answered; one for a query the user has
    /// since edited away from is stale and dropped.
    pub fn on_results(&mut self, response: PromptSearchResponse) -> bool {
        if response.query != self.query.text() {
            return false;
        }
        self.replace_results(response.results);
        self.loading = false;
        self.error = None;
        true
    }

    pub fn on_failed(&mut self, query: &str, error: String) -> bool {
        if query != self.query.text() {
            return false;
        }
        self.replace_results(Vec::new());
        self.loading = false;
        self.error = Some(error);
        true
    }

    fn refresh_query_state(&mut self) {
        self.error = None;
        if self.query.text().trim().is_empty() {
            self.replace_results(Vec::new());
            self.loading = false;
        } else {
            self.loading = true;
        }
    }

    /// Appends `c` to the query, returning the query it produced.
    pub fn push_char(&mut self, c: char) -> String {
        self.query.insert_char(c);
        self.refresh_query_state();
        self.query.text().to_string()
    }

    /// Appends the printable part of `text` to the query, returning the query it
    /// produced. Pastes that are entirely control characters leave it unchanged.
    pub fn push_str(&mut self, text: &str) -> String {
        let sanitized: String = text.chars().filter(|c| !c.is_control()).collect();
        if !sanitized.is_empty() {
            self.query.insert_str(&sanitized);
            self.refresh_query_state();
        }
        self.query.text().to_string()
    }

    pub fn backspace(&mut self) -> String {
        self.query.backspace();
        self.refresh_query_state();
        self.query.text().to_string()
    }

    pub fn height(&self, max_rows: usize) -> usize {
        1 + if self.error.is_some() || self.query.text().trim().is_empty() || self.results.is_empty() {
            1
        } else {
            self.results.filtered_len().min(max_rows)
        }
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        if area.is_empty() {
            return;
        }
        let [header_area, results_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(0),
        ])
        .areas(area);
        Paragraph::new(Line::styled(format!("history search: {}", self.query.text()), Style::new().fg(theme.info)))
            .render(header_area, buf);

        let message = self.error.as_deref().map(|error| format!("  error: {error}"));
        let message = message.or_else(|| {
            if self.query.text().trim().is_empty() {
                Some("  type to search prompt history".to_string())
            } else if self.loading && self.results.is_empty() {
                Some("  searching…".to_string())
            } else if self.results.is_empty() {
                Some("  no matching prompts".to_string())
            } else {
                None
            }
        });
        if let Some(message) = message {
            Paragraph::new(message).style(Style::new().fg(theme.muted)).render(results_area, buf);
            return;
        }

        let width = usize::from(results_area.width.max(1));
        let (view, selection) = self.results.view(theme, |result| result_line(result, width, theme));
        StatefulWidget::render(view, results_area, buf, selection);
    }

    fn result_list(results: Vec<PromptSearchResult>) -> FilterableList<PromptSearchResult> {
        // The agent already filters and ranks these results. Keep the shared list's
        // query empty so it supplies selection and scrolling without re-filtering.
        FilterableList::new(results, |result| result.prompt.clone())
    }

    /// Replacing server-filtered results resets focus to the first result. This
    /// keeps response replacement and clearing consistent with query changes.
    fn replace_results(&mut self, results: Vec<PromptSearchResult>) {
        self.results = Self::result_list(results);
    }
}

fn result_line(result: &PromptSearchResult, max_width: usize, theme: &Theme) -> Line<'static> {
    let cwd_display = match home_relative_path(&result.cwd) {
        full if full.width() <= MAX_CWD_WIDTH => full,
        full => result
            .cwd
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| name.width() <= MAX_CWD_WIDTH)
            .unwrap_or(full),
    };
    let cwd_width = cwd_display.width();
    let shows_cwd = cwd_width > 0 && max_width >= cwd_width + CWD_GAP + MIN_PROMPT_WIDTH;
    let prompt_budget = if shows_cwd { max_width - cwd_width - CWD_GAP } else { max_width };

    let prompt = prompt_spans(
        &result.prompt,
        result.match_start..result.match_end,
        Style::new().fg(theme.text_secondary),
        Style::new().fg(theme.warning),
    );
    let mut spans = truncate_spans(&prompt, prompt_budget, Style::default());
    if shows_cwd {
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::raw(" ".repeat(prompt_budget + CWD_GAP - used)));
        spans.push(Span::styled(cwd_display, Style::new().fg(theme.muted)));
    }
    Line::from(spans)
}

/// The prompt as alternating matched/unmatched runs.
///
/// Runs of whitespace collapse to a single space so a multi-line prompt reads as
/// one row; the space counts as matched when any character it stands in for did.
fn prompt_spans(prompt: &str, matched: Range<usize>, base: Style, highlight: Style) -> Vec<Span<'static>> {
    let mut characters: Vec<(char, bool)> = Vec::new();
    let mut last_was_whitespace = false;
    for (index, character) in prompt.char_indices() {
        let in_match = matched.contains(&index);
        if !character.is_whitespace() {
            last_was_whitespace = false;
            characters.push((character, in_match));
            continue;
        }
        match characters.last_mut() {
            Some((_, previous)) if last_was_whitespace => *previous = *previous || in_match,
            _ => {
                last_was_whitespace = true;
                characters.push((' ', in_match));
            }
        }
    }

    characters
        .chunk_by(|left, right| left.1 == right.1)
        .map(|run| {
            let style = if run[0].1 { highlight } else { base };
            Span::styled(run.iter().map(|(character, _)| *character).collect::<String>(), style)
        })
        .collect()
}

pub fn cursor_at_match_end(prompt: &str, match_end: usize) -> usize {
    let mut idx = match_end.min(prompt.len());
    while idx > 0 && !prompt.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
