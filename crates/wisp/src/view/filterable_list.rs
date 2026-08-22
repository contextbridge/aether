use crate::view::list_view::ListView;
use crate::view::selection::{Direction, SelectionState};
use crate::theme::Theme;
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;

/// A list of `T` with a fuzzy-match filter, a persistent selection, and the
/// navigation policy that selection moves under.
///
/// Holding the policy here keeps filtering and selection behavior with the
/// reusable list instead of duplicating it in each picker.
#[derive(Clone, Debug)]
pub struct FilterableList<T> {
    entries: Vec<T>,
    match_keys: Vec<String>,
    query: String,
    filtered_indices: Vec<usize>,
    selection: SelectionState,
    /// Entries navigation may land on; the rest are stepped over.
    selectable: fn(&T) -> bool,
    /// Whether stepping wraps at the ends. Long lists that are scanned rather
    /// than cycled stop instead.
    wraps: bool,
    preserve_order: bool,
}

impl<T> FilterableList<T> {
    pub fn new(entries: Vec<T>, match_key: impl Fn(&T) -> String) -> Self {
        let match_keys = entries.iter().map(&match_key).collect();
        let filtered_indices = (0..entries.len()).collect();
        let selection = SelectionState::new(entries.len());
        Self {
            entries,
            match_keys,
            query: String::new(),
            filtered_indices,
            selection,
            selectable: |_| true,
            wraps: true,
            preserve_order: false,
        }
    }

    /// Restricts navigation to the entries `selectable` accepts, so disabled
    /// rows are never focused.
    pub fn selectable(mut self, selectable: fn(&T) -> bool) -> Self {
        self.selectable = selectable;
        self
    }

    /// Stops navigation at the ends instead of wrapping.
    pub fn clamped(mut self) -> Self {
        self.wraps = false;
        self
    }

    /// Keeps filtered entries in source order instead of ranking matches.
    pub fn preserve_order(mut self) -> Self {
        self.preserve_order = true;
        self
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

    pub fn selected_entry(&self) -> Option<&T> {
        self.selection.selected().and_then(|selected| self.filtered_indices.get(selected)).copied().map(|index| &self.entries[index])
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
        let previous = self.selection.selected();
        let hit = self.selection.select_at(row, self.filtered_len())
            && self.selected_entry().is_some_and(self.selectable);
        if !hit {
            self.selection.select(previous, self.filtered_len());
        }
        self.clear_offset_when_empty();
        hit
    }

    /// Moves to the nearest filtered entry in `direction` that this list's
    /// policy allows the selection to rest on.
    pub fn step(&mut self, direction: Direction) {
        let Self { entries, filtered_indices, selection, selectable, wraps, .. } = self;
        let allowed = |row: usize| selectable(&entries[filtered_indices[row]]);
        if *wraps {
            selection.step(filtered_indices.len(), direction, allowed);
        } else {
            selection.step_clamped(filtered_indices.len(), direction, allowed);
        }
    }

    pub fn select_index(&mut self, index: usize) {
        self.selection.select(
            self.filtered_indices.iter().position(|&filtered_index| filtered_index == index),
            self.filtered_len(),
        );
    }

    /// The matching entries as a [`ListView`] and its persistent selection state.
    /// Rows run only for entries actually drawn, so filtering a list of every
    /// file in the working tree does not format one row per match.
    pub fn view<'a>(
        &'a mut self,
        theme: &'a Theme,
        mut row: impl FnMut(&T) -> Line<'static> + 'a,
    ) -> (ListView<'a>, &'a mut SelectionState) {
        let Self { entries, filtered_indices, selection, .. } = self;
        let (entries, filtered_indices) = (&*entries, &*filtered_indices);
        let view = ListView::lazy(filtered_indices.len(), move |index| row(&entries[filtered_indices[index]]), theme);
        (view, selection)
    }

    /// Draws the full-pane picker chrome every typeahead picker shares: a
    /// titled border showing the active query, a scrollbar, and `empty` when
    /// nothing matches.
    pub fn render_pane(
        &mut self,
        label: &str,
        empty: &'static str,
        area: Rect,
        buf: &mut Buffer,
        theme: &Theme,
        row: impl FnMut(&T) -> Line<'static>,
    ) {
        let title = if self.query.is_empty() { format!(" {label} ") } else { format!(" {label} '{}' ", self.query) };
        let (view, selection) = self.view(theme, row);
        let view = view.empty_message(empty).bordered(title).scrollbar();
        StatefulWidget::render(view, area, buf, selection);
    }

    fn refilter(&mut self) {
        if self.query.is_empty() {
            self.filtered_indices = (0..self.entries.len()).collect();
        } else {
            self.filtered_indices = fuzzy_filter(&self.query, &self.match_keys);
            if self.preserve_order {
                self.filtered_indices.sort_unstable();
            }
        }
        self.selection.select_first(self.filtered_len());
        if self.selected_entry().is_some_and(|entry| !(self.selectable)(entry)) {
            self.step(Direction::Forward);
        }
        self.clear_offset_when_empty();
    }

    fn clear_offset_when_empty(&mut self) {
        if self.filtered_indices.is_empty() {
            self.selection.set_offset(0);
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
