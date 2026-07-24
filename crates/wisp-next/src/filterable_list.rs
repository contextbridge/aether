use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, List, ListItem, ListState, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};
use unicode_width::UnicodeWidthStr;

pub struct FilterableListRender<'a> {
    pub title: String,
    pub empty_message: &'a str,
    pub border_style: Style,
    pub empty_style: Style,
    pub highlight_style: Style,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterableList<T> {
    entries: Vec<T>,
    match_keys: Vec<String>,
    query: String,
    filtered_indices: Vec<usize>,
    state: ListState,
}

impl<T> FilterableList<T> {
    pub fn new(entries: Vec<T>, match_key: impl Fn(&T) -> String) -> Self {
        let match_keys = entries.iter().map(&match_key).collect();
        let filtered_indices = (0..entries.len()).collect();
        let state = ListState::default().with_selected((!entries.is_empty()).then_some(0));
        Self { entries, match_keys, query: String::new(), filtered_indices, state }
    }

    pub fn entries(&self) -> &[T] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [T] {
        &mut self.entries
    }

    pub fn filtered_entries(&self) -> impl Iterator<Item = (usize, &T)> {
        self.filtered_indices.iter().copied().map(|index| (index, &self.entries[index]))
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn filtered_len(&self) -> usize {
        self.filtered_indices.len()
    }

    pub fn offset(&self) -> usize {
        self.state.offset()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.state.selected().and_then(|selected| self.filtered_indices.get(selected)).copied()
    }

    pub fn selected_entry(&self) -> Option<&T> {
        self.selected_index().map(|index| &self.entries[index])
    }

    pub fn push_query_char(&mut self, character: char) {
        self.query.push(character);
        self.refilter();
    }

    pub fn pop_query_char(&mut self) {
        self.query.pop();
        self.refilter();
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        let query = query.into();
        if self.query == query {
            return;
        }

        self.query = query;
        self.refilter();
    }

    fn refilter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.entries.len()).collect();
        } else {
            self.filtered_indices = fuzzy_filter(&self.query, &self.match_keys);
        }
        self.select_first();
    }

    pub fn select_row(&mut self, row: usize) {
        self.select(self.state.offset().checked_add(row));
    }

    pub fn select_previous(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.state.select(None);
            return;
        }
        let selected = self.state.selected().unwrap_or_default();
        self.state.select(Some(selected.checked_sub(1).unwrap_or(len - 1)));
    }

    pub fn select_next(&mut self) {
        let len = self.filtered_len();
        if len == 0 {
            self.state.select(None);
            return;
        }
        self.state.select(Some((self.state.selected().unwrap_or_default() + 1) % len));
    }

    pub fn select_index(&mut self, index: usize) {
        self.state.select(self.filtered_indices.iter().position(|&filtered_index| filtered_index == index));
    }

    pub fn render_items(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        empty_message: &str,
        empty_style: Style,
        highlight_style: Style,
        mut item: impl FnMut(&T, usize) -> ListItem<'static>,
    ) {
        if self.filtered_indices.is_empty() {
            ratatui::widgets::Paragraph::new(empty_message).style(empty_style).render(area, buf);
            return;
        }

        let items = self.filtered_entries().map(|(index, entry)| item(entry, index));
        let list = List::new(items).highlight_style(highlight_style).scroll_padding(1);
        StatefulWidget::render(list, area, buf, &mut self.state);
    }

    pub fn render(
        &mut self,
        area: Rect,
        buf: &mut Buffer,
        config: FilterableListRender<'_>,
        item: impl FnMut(&T, usize) -> ListItem<'static>,
    ) {
        let block = Block::bordered().title(config.title).style(config.border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        self.render_items(inner, buf, config.empty_message, config.empty_style, config.highlight_style, item);
        let mut scrollbar_state = ScrollbarState::new(self.filtered_len()).position(self.state.offset());
        StatefulWidget::render(Scrollbar::new(ScrollbarOrientation::VerticalRight), inner, buf, &mut scrollbar_state);
    }

    /// Renders the filtered entries as styled lines for inline overlays (e.g.
    /// command/file pickers shown above the composer).
    pub fn inline_lines(
        &self,
        width: u16,
        max_rows: usize,
        theme: &Theme,
        empty_message: &str,
        label: impl Fn(&T) -> String,
    ) -> Vec<Line<'static>> {
        let width = usize::from(width.max(1));
        let mut lines = vec![Line::styled("─".repeat(width), Style::new().fg(theme.muted))];
        if self.filtered_indices.is_empty() {
            lines.push(Line::styled(format!("  ({empty_message})"), Style::new().fg(theme.muted)));
        } else {
            let selected = self.state.selected().unwrap_or_default();
            let start = selected.saturating_sub(max_rows.saturating_sub(1));
            for (row, &index) in self.filtered_indices.iter().enumerate().skip(start).take(max_rows) {
                let value = truncate_to_width(&label(&self.entries[index]), width.saturating_sub(2));
                let is_selected = row == selected;
                let style = if is_selected {
                    Style::new().fg(theme.text_primary).bg(theme.sidebar_bg)
                } else {
                    Style::new().fg(theme.text_secondary)
                };
                let text = format!("  {value}");
                let padding = " ".repeat(width.saturating_sub(text.width()));
                lines.push(Line::from(vec![Span::styled(text, style), Span::styled(padding, style)]));
            }
        }
        lines
    }

    fn select_first(&mut self) {
        self.state.select((!self.filtered_indices.is_empty()).then_some(0));
        if self.filtered_indices.is_empty() {
            *self.state.offset_mut() = 0;
        }
    }

    fn select(&mut self, selected: Option<usize>) {
        self.state.select(selected.filter(|index| *index < self.filtered_len()));
        if self.filtered_indices.is_empty() {
            *self.state.offset_mut() = 0;
        }
    }
}

/// Fuzzy-filters `haystacks` against `query` using nucleo, returning matching
/// indices sorted by match quality (best first).
fn fuzzy_filter(query: &str, haystacks: &[String]) -> Vec<usize> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, usize)> = haystacks
        .iter()
        .enumerate()
        .filter_map(|(index, haystack)| {
            let utf32 = Utf32Str::new(haystack, &mut buf);
            pattern.score(utf32, &mut matcher).map(|score| (score, index))
        })
        .collect();
    scored.sort_by_key(|&(score, _)| std::cmp::Reverse(score));
    scored.into_iter().map(|(_, index)| index).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_filter_ranks_by_match_quality() {
        let haystacks = vec!["alphabet".to_string(), "alpha".to_string(), "gamma".to_string()];
        let result = fuzzy_filter("alp", &haystacks);
        assert!(result.contains(&0));
        assert!(result.contains(&1));
        assert!(!result.contains(&2));
    }

    #[test]
    fn fuzzy_filter_empty_query_returns_all() {
        let haystacks = vec!["a".to_string(), "b".to_string()];
        let result = fuzzy_filter("", &haystacks);
        assert_eq!(result, vec![0, 1]);
    }

    #[test]
    fn fuzzy_filter_case_insensitive() {
        let haystacks = vec!["Alpha".to_string(), "beta".to_string()];
        let result = fuzzy_filter("alp", &haystacks);
        assert_eq!(result, vec![0]);
    }
}
