use crate::list_view::ListView;
use crate::selection::{Direction, SelectionState};
use crate::surface::ListFilter;
use crate::theme::Theme;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use ratatui::text::Line;

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

    /// The first entry currently scrolled into view.
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

    /// The matching entries as [`ListView`] rows, each built by `row`. Decorate
    /// the result with the usual [`ListView`] chrome.
    ///
    /// `row` runs only for the entries actually drawn, so filtering a list of
    /// every file in the working tree does not format one row per match.
    pub fn view<'a>(&'a mut self, theme: &'a Theme, mut row: impl FnMut(&T) -> Line<'static> + 'a) -> ListView<'a> {
        let Self { entries, filtered_indices, selection, .. } = self;
        let (entries, filtered_indices) = (&*entries, &*filtered_indices);
        ListView::lazy(filtered_indices.len(), move |index| row(&entries[filtered_indices[index]]), selection, theme)
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
