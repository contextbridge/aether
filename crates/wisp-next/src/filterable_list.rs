use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{
    Block, List, ListItem, Scrollbar, ScrollbarOrientation, ScrollbarState, StatefulWidget, Widget,
};
use std::cell::RefCell;

pub struct FilterableListRender<'a> {
    pub title: String,
    pub empty_message: &'a str,
    pub border_style: Style,
    pub empty_style: Style,
    pub highlight_style: Style,
}

pub struct FilterableList<T> {
    entries: Vec<T>,
    match_key: Box<dyn Fn(&T) -> String>,
    query: String,
    filtered_indices: Vec<usize>,
    state: RefCell<ratatui::widgets::ListState>,
}

impl<T> FilterableList<T> {
    pub fn new(entries: Vec<T>, match_key: impl Fn(&T) -> String + 'static) -> Self {
        let filtered_indices = (0..entries.len()).collect();
        let state = ratatui::widgets::ListState::default().with_selected((!entries.is_empty()).then_some(0));
        Self {
            entries,
            match_key: Box::new(match_key),
            query: String::new(),
            filtered_indices,
            state: RefCell::new(state),
        }
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

    pub fn selected_index(&self) -> Option<usize> {
        self.state.borrow().selected().and_then(|selected| self.filtered_indices.get(selected)).copied()
    }

    pub fn selected_entry(&self) -> Option<&T> {
        self.selected_index().map(|index| &self.entries[index])
    }

    pub fn set_query(&mut self, query: impl Into<String>) {
        let query = query.into();
        if self.query == query {
            return;
        }

        self.query = query;
        self.filtered_indices = if self.query.is_empty() {
            (0..self.entries.len()).collect()
        } else {
            let query = self.query.to_ascii_lowercase();
            self.entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    ((self.match_key)(entry).to_ascii_lowercase().contains(&query)).then_some(index)
                })
                .collect()
        };
        self.select_first();
    }

    pub fn select_row(&mut self, row: usize) {
        let selected = self.state.borrow().offset().checked_add(row);
        self.select(selected);
    }

    pub fn select_previous(&mut self) {
        let len = self.filtered_len();
        let mut state = self.state.borrow_mut();
        if len == 0 {
            state.select(None);
            return;
        }
        let selected = state.selected().unwrap_or_default();
        state.select(Some(selected.checked_sub(1).unwrap_or(len - 1)));
    }

    pub fn select_next(&mut self) {
        let len = self.filtered_len();
        let mut state = self.state.borrow_mut();
        if len == 0 {
            state.select(None);
            return;
        }
        let selected = (state.selected().unwrap_or_default() + 1) % len;
        state.select(Some(selected));
    }

    pub fn render(
        &self,
        area: Rect,
        buf: &mut Buffer,
        config: FilterableListRender<'_>,
        mut item: impl FnMut(&T, usize) -> ListItem<'static>,
    ) {
        let block = Block::bordered().title(config.title).style(config.border_style);
        let inner = block.inner(area);
        block.render(area, buf);

        if self.filtered_indices.is_empty() {
            ratatui::widgets::Paragraph::new(config.empty_message).style(config.empty_style).render(inner, buf);
            return;
        }

        let items = self.filtered_entries().map(|(index, entry)| item(entry, index));
        let list = List::new(items).highlight_style(config.highlight_style);
        let mut state = self.state.borrow_mut();
        StatefulWidget::render(list, inner, buf, &mut state);
        let mut scrollbar_state = ScrollbarState::new(self.filtered_len()).position(state.offset());
        StatefulWidget::render(Scrollbar::new(ScrollbarOrientation::VerticalRight), inner, buf, &mut scrollbar_state);
    }

    fn select_first(&mut self) {
        let mut state = self.state.borrow_mut();
        state.select((!self.filtered_indices.is_empty()).then_some(0));
        if self.filtered_indices.is_empty() {
            *state.offset_mut() = 0;
        }
    }

    fn select(&mut self, selected: Option<usize>) {
        let mut state = self.state.borrow_mut();
        state.select(selected.filter(|index| *index < self.filtered_len()));
        if self.filtered_indices.is_empty() {
            *state.offset_mut() = 0;
        }
    }
}
