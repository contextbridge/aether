use crate::theme::Theme;
use crate::view::edit_buffer::is_composed_char;
use crate::view::input::{Nav, UiEvent};
use crate::view::list_view::ListView;
use crate::view::selection::{Direction, SelectionState};
use clankerdiff_ui_core::{PickerOption, PickerState};
use crossterm::event::KeyCode;
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
    picker: PickerState,
    query: String,
    filtered_indices: Vec<usize>,
    selection: SelectionState,
    /// Entries navigation may land on; the rest are stepped over.
    selectable: fn(&T) -> bool,
    preserve_order: bool,
}

impl<T> FilterableList<T> {
    pub fn new(entries: Vec<T>, match_key: impl Fn(&T) -> String) -> Self {
        let picker = PickerState::new(entries.iter().map(|entry| PickerOption::new(match_key(entry))).collect());
        let filtered_indices = picker.matches().to_vec();
        let selection = SelectionState::new(entries.len());
        Self {
            entries,
            picker,
            query: String::new(),
            filtered_indices,
            selection,
            selectable: |_| true,
            preserve_order: false,
        }
    }

    /// Restricts navigation to the entries `selectable` accepts, so disabled
    /// rows are never focused.
    pub fn selectable(mut self, selectable: fn(&T) -> bool) -> Self {
        self.selectable = selectable;
        self.normalize_selection();
        self
    }

    /// Stops navigation at the ends instead of wrapping.
    pub fn clamped(mut self) -> Self {
        self.selection = self.selection.clamped();
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
        self.selection
            .selected()
            .and_then(|selected| self.filtered_indices.get(selected))
            .copied()
            .map(|index| &self.entries[index])
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
        let (selection, len, selectable) = self.navigation();
        selection.select_at(row, len, selectable)
    }

    /// Moves to the nearest filtered entry in `direction` that this list's
    /// policy allows the selection to rest on.
    pub fn step(&mut self, direction: Direction) {
        let (selection, len, selectable) = self.navigation();
        selection.step(len, direction, selectable);
    }

    pub(crate) fn on_nav_event(&mut self, event: &UiEvent) -> Nav {
        if let UiEvent::Key(key) = event {
            match key.code {
                KeyCode::Backspace => {
                    self.pop_query_char();
                    return Nav::Moved;
                }
                KeyCode::Char(c) if !c.is_control() && !is_composed_char(*key) => {
                    self.push_query_char(c);
                    return Nav::Moved;
                }
                _ => {}
            }
        }
        self.navigate(event)
    }

    pub(crate) fn navigate(&mut self, event: &UiEvent) -> Nav {
        let (selection, len, selectable) = self.navigation();
        selection.on_nav_event(event, len, selectable)
    }

    pub fn select_index(&mut self, index: usize) {
        self.selection.select(
            self.filtered_indices.iter().position(|&filtered_index| filtered_index == index),
            self.filtered_len(),
        );
        self.normalize_selection();
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

    fn navigation(&mut self) -> (&mut SelectionState, usize, impl Fn(usize) -> bool + '_) {
        let Self { entries, filtered_indices, selection, selectable, .. } = self;
        let (entries, filtered_indices, selectable) = (&*entries, &*filtered_indices, *selectable);
        (selection, filtered_indices.len(), move |row| selectable(&entries[filtered_indices[row]]))
    }

    fn refilter(&mut self) {
        self.picker.set_query(self.query.as_str());
        self.filtered_indices = self.picker.matches().to_vec();
        if self.preserve_order {
            self.filtered_indices.sort_unstable();
        }
        self.selection.select_first(self.filtered_len());
        self.normalize_selection();
        self.clear_offset_when_empty();
    }

    fn normalize_selection(&mut self) {
        if self.selected_entry().is_some_and(self.selectable) {
            return;
        }
        let selected = self.filtered_indices.iter().position(|&index| (self.selectable)(&self.entries[index]));
        self.selection.select(selected, self.filtered_len());
    }

    fn clear_offset_when_empty(&mut self) {
        if self.filtered_indices.is_empty() {
            self.selection.set_offset(0);
        }
    }
}
