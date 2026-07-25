use crate::selection::{Direction, SelectionState};
use crate::surface::ListFilter;
use crate::theme::Theme;
use crate::widgets::render_vertical_scrollbar;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Block, List, ListItem, Paragraph, StatefulWidget, Widget};

/// A list of `T` with a fuzzy-match filter and a persistent selection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterableList<T> {
    entries: Vec<T>,
    match_keys: Vec<String>,
    query: String,
    filtered_indices: Vec<usize>,
    selection: SelectionState,
}

impl<T> FilterableList<T> {
    pub fn new(entries: Vec<T>, match_key: impl Fn(&T) -> String) -> Self {
        let match_keys = entries.iter().map(&match_key).collect();
        let filtered_indices = (0..entries.len()).collect();
        let selection = SelectionState::new(entries.len());
        Self { entries, match_keys, query: String::new(), filtered_indices, selection }
    }

    pub fn entries(&self) -> &[T] {
        &self.entries
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
        self.selection.offset()
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selection.selected().and_then(|selected| self.filtered_indices.get(selected)).copied()
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

    /// Selects the entry drawn at terminal `row`, reporting whether one was hit.
    pub fn select_at(&mut self, row: u16) -> bool {
        let hit = self.selection.select_at(row, self.filtered_len());
        self.clear_offset_when_empty();
        hit
    }

    pub fn select_previous(&mut self) {
        self.step(Direction::Backward, |_| true);
    }

    pub fn select_next(&mut self) {
        self.step(Direction::Forward, |_| true);
    }

    /// Wrapping move to the nearest filtered entry in `direction` satisfying
    /// `selectable`, so disabled rows are never focused.
    pub fn step(&mut self, direction: Direction, selectable: impl Fn(&T) -> bool) {
        let Self { entries, filtered_indices, selection, .. } = self;
        selection.step(filtered_indices.len(), direction, |row| selectable(&entries[filtered_indices[row]]));
    }

    /// Block title for a picker pane, showing the active query when there is one.
    pub fn search_title(&self, label: &str) -> String {
        if self.query.is_empty() { format!(" {label} ") } else { format!(" {label} '{}' ", self.query) }
    }

    /// Like [`Self::step`], but stops at the ends instead of wrapping.
    pub fn step_clamped(&mut self, direction: Direction, selectable: impl Fn(&T) -> bool) {
        let Self { entries, filtered_indices, selection, .. } = self;
        selection.step_clamped(filtered_indices.len(), direction, |row| selectable(&entries[filtered_indices[row]]));
    }

    pub fn select_index(&mut self, index: usize) {
        self.selection.select(
            self.filtered_indices.iter().position(|&filtered_index| filtered_index == index),
            self.filtered_len(),
        );
    }

    /// Renders the filtered entries as a ratatui [`List`], rows produced by
    /// `item`. Decorate with [`FilterableListView::block`] and
    /// [`FilterableListView::scrollbar`].
    pub fn view<'a, F>(&'a mut self, theme: &'a Theme, empty_message: &'a str, item: F) -> FilterableListView<'a, T, F>
    where
        F: FnMut(&T) -> ListItem<'static>,
    {
        FilterableListView { list: self, theme, empty_message, item, block: None, scrollbar: false, highlight: None }
    }

    fn refilter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.entries.len()).collect();
        } else {
            self.filtered_indices = fuzzy_filter(&self.query, &self.match_keys);
        }
        self.selection.select_first(self.filtered_len());
        self.clear_offset_when_empty();
    }

    fn clear_offset_when_empty(&mut self) {
        if self.filtered_indices.is_empty() {
            *self.selection.list_state_mut().offset_mut() = 0;
        }
    }
}

impl<T> ListFilter for FilterableList<T> {
    fn push_query_char(&mut self, character: char) {
        FilterableList::push_query_char(self, character);
    }

    fn pop_query_char(&mut self) {
        FilterableList::pop_query_char(self);
    }
}

/// A [`FilterableList`] rendered as a ratatui widget.
pub struct FilterableListView<'a, T, F> {
    list: &'a mut FilterableList<T>,
    theme: &'a Theme,
    empty_message: &'a str,
    item: F,
    block: Option<Block<'static>>,
    scrollbar: bool,
    highlight: Option<Style>,
}

impl<T, F> FilterableListView<'_, T, F> {
    pub fn block(mut self, block: Block<'static>) -> Self {
        self.block = Some(block);
        self
    }

    /// Wraps the list in a titled border, the standard full-pane picker chrome.
    pub fn bordered(self, title: impl Into<String>) -> Self {
        let style = Style::new().fg(self.theme.text_primary);
        self.block(Block::bordered().title(title.into()).style(style)).scrollbar()
    }

    pub fn scrollbar(mut self) -> Self {
        self.scrollbar = true;
        self
    }

    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight = Some(style);
        self
    }
}

impl<T, F> Widget for FilterableListView<'_, T, F>
where
    F: FnMut(&T) -> ListItem<'static>,
{
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Self { list, theme, empty_message, mut item, block, scrollbar, highlight } = self;
        let inner = block.as_ref().map_or(area, |block| block.inner(area));
        if let Some(block) = block {
            block.render(area, buf);
        }

        if list.filtered_indices.is_empty() {
            Paragraph::new(empty_message).style(Style::new().fg(theme.muted)).render(inner, buf);
            return;
        }

        let rows: Vec<ListItem<'static>> =
            list.filtered_indices.iter().map(|&index| item(&list.entries[index])).collect();
        let highlight = highlight.unwrap_or_else(|| Style::new().fg(theme.text_primary).bg(theme.sidebar_bg));
        let widget = List::new(rows).highlight_style(highlight).scroll_padding(1);
        list.selection.set_rows_area(inner);
        StatefulWidget::render(widget, inner, buf, list.selection.list_state_mut());

        if scrollbar {
            render_vertical_scrollbar(inner, buf, list.filtered_indices.len(), list.selection.offset());
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
