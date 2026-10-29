use crate::theme::Theme;
use crate::view::input::{Nav, UiEvent};
use crate::view::list_view::ListView;
use crate::view::selection::SelectionState;
use crate::view::wrap::truncate_spans;
use acp_utils::notifications::PromptSearchResult;
use client_core::composer::prompt_search::{PromptSearchState, PromptSearchStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};

use std::ops::Range;
use unicode_width::UnicodeWidthStr;

const MAX_CWD_WIDTH: usize = 32;
const CWD_GAP: usize = 2;
const MIN_PROMPT_WIDTH: usize = 16;

/// The cursor over the session's prompt-history results. The query and the
/// results themselves live in [`PromptSearchState`], already ranked by the
/// agent.
#[derive(Debug, Default)]
pub struct PromptSearchPicker {
    selection: SelectionState,
}

impl PromptSearchPicker {
    pub fn selected_result<'a>(&self, search: &'a PromptSearchState) -> Option<&'a PromptSearchResult> {
        self.selection.selected().and_then(|selected| search.results().get(selected))
    }

    pub(crate) fn navigate(&mut self, event: &UiEvent, search: &PromptSearchState) -> Nav {
        self.selection.on_nav_event(event, search.results().len(), |_| true)
    }

    /// Starts over at the first of a fresh set of results.
    pub fn on_results(&mut self, search: &PromptSearchState) {
        self.selection = SelectionState::new(search.results().len());
    }

    /// Rows the open picker wants: its header, then the results or a status line.
    pub fn height(search: &PromptSearchState, max_rows: usize) -> usize {
        1 + match search.status() {
            PromptSearchStatus::Results => search.results().len().min(max_rows),
            _ => 1,
        }
    }

    pub fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme, search: &PromptSearchState) {
        if area.is_empty() {
            return;
        }
        let [header_area, results_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(0),
        ])
        .areas(area);
        Paragraph::new(Line::styled(format!("history search: {}", search.query()), theme.info))
            .render(header_area, buf);

        let message = match search.status() {
            PromptSearchStatus::Failed(error) => format!("  error: {error}"),
            PromptSearchStatus::Blank => "  type to search prompt history".to_owned(),
            PromptSearchStatus::Searching => "  searching…".to_owned(),
            PromptSearchStatus::NoMatches => "  no matching prompts".to_owned(),
            PromptSearchStatus::Results => {
                let width = usize::from(results_area.width.max(1));
                let results = search.results();
                let view = ListView::lazy(results.len(), |index| result_line(&results[index], width, theme), theme);
                StatefulWidget::render(view, results_area, buf, &mut self.selection);
                return;
            }
        };
        Paragraph::new(message).style(theme.muted).render(results_area, buf);
    }
}

fn result_line(result: &PromptSearchResult, max_width: usize, theme: &Theme) -> Line<'static> {
    let cwd_display = match result.cwd.display().to_string() {
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

    let prompt =
        prompt_spans(&result.prompt, result.match_start..result.match_end, theme.text_secondary, theme.warning);
    let mut spans = truncate_spans(&prompt, prompt_budget, Style::default());
    if shows_cwd {
        let used: usize = spans.iter().map(Span::width).sum();
        spans.push(Span::raw(" ".repeat(prompt_budget + CWD_GAP - used)));
        spans.push(Span::styled(cwd_display, theme.muted));
    }
    Line::from(spans)
}

/// The prompt as alternating matched/unmatched runs.
///
/// Runs of whitespace collapse to a single space so a multi-line prompt reads as
/// one row; the space counts as matched when any character it stands in for did.
fn prompt_spans(prompt: &str, matched: Range<usize>, base: Color, highlight: Color) -> Vec<Span<'static>> {
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
