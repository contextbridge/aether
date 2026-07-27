use crate::edit_buffer::EditBuffer;
use crate::generation::Generation;
use crate::list_view::ListView;
use crate::selection::{Direction, SelectionState};
use crate::theme::Theme;
use crate::workspace_status::home_relative_path;
use crate::wrap::truncate_spans;
use acp_utils::notifications::{PromptSearchResponse, PromptSearchResult};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use std::ops::Range;
use std::path::Path;
use unicode_width::UnicodeWidthStr;

const MAX_CWD_WIDTH: usize = 32;
const CWD_GAP: usize = 2;
const MIN_PROMPT_WIDTH: usize = 16;

#[derive(Debug, Default)]
pub struct PromptSearchPicker {
    query: EditBuffer,
    results: Vec<PromptSearchResult>,
    selection: SelectionState,
    loading: bool,
    error: Option<String>,
    search_generation: Generation,
}

impl PromptSearchPicker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query(&self) -> &str {
        self.query.text()
    }

    pub fn selected_result(&self) -> Option<&PromptSearchResult> {
        self.selection.selected().and_then(|selected| self.results.get(selected))
    }

    pub fn search_generation(&self) -> Generation {
        self.search_generation
    }

    pub fn on_results(&mut self, response: PromptSearchResponse) -> bool {
        if response.query != self.query.text() || Generation::from(response.search_generation) != self.search_generation
        {
            return false;
        }
        self.results = response.results;
        self.selection.select_first(self.results.len());
        self.loading = false;
        self.error = None;
        true
    }

    pub fn on_failed(&mut self, search_generation: Generation, error: String) -> bool {
        if search_generation != self.search_generation {
            return false;
        }
        self.results.clear();
        self.selection.select_first(self.results.len());
        self.loading = false;
        self.error = Some(error);
        true
    }

    fn refresh_query_state(&mut self) {
        self.error = None;
        if self.query.text().trim().is_empty() {
            self.results.clear();
            self.selection.select_first(self.results.len());
            self.loading = false;
        } else {
            self.search_generation.bump();
            self.loading = true;
        }
    }

    pub fn step(&mut self, direction: Direction) {
        self.selection.step(self.results.len(), direction, |_| true);
    }

    /// Selects the result drawn at terminal `row`, if one is there.
    pub fn select_at(&mut self, row: u16) {
        self.selection.select_at(row, self.results.len());
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
            self.results.len().min(max_rows)
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

        let message = self.error.as_ref().map(|error| format!("  error: {error}")).or_else(|| {
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
        let rows = self.results.iter().map(|result| result_line(result, width, theme)).collect();
        ListView::new(rows, &mut self.selection, theme).render(results_area, buf);
    }
}

/// One result row: the prompt with its matched span highlighted, and the
/// originating directory pushed to the right when there is room for it.
fn result_line(result: &PromptSearchResult, max_width: usize, theme: &Theme) -> Line<'static> {
    let cwd_display = abbreviate_cwd(&result.cwd, MAX_CWD_WIDTH);
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

fn abbreviate_cwd(cwd: &Path, max_width: usize) -> String {
    let full = home_relative_path(cwd);
    if full.width() <= max_width {
        return full;
    }
    cwd.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| name.width() <= max_width)
        .unwrap_or(full)
}

pub fn cursor_at_match_end(prompt: &str, match_end: usize) -> usize {
    let mut idx = match_end.min(prompt.len());
    while idx > 0 && !prompt.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}
