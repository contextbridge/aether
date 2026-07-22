use ratatui::widgets::ListState;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectionState {
    list: ListState,
}

impl SelectionState {
    pub fn new(len: usize) -> Self {
        let mut state = Self::default();
        state.select_first(len);
        state
    }

    pub fn ensure_visible(&mut self, len: usize, visible_rows: usize) {
        self.clamp(len);
        let Some(selected) = self.selected() else {
            return;
        };
        if selected < self.offset() {
            *self.list.offset_mut() = selected;
        } else if visible_rows > 0 && selected >= self.offset().saturating_add(visible_rows) {
            *self.list.offset_mut() = selected + 1 - visible_rows;
        }
    }

    pub fn selected(&self) -> Option<usize> {
        self.list.selected()
    }

    pub fn offset(&self) -> usize {
        self.list.offset()
    }

    pub fn select(&mut self, selected: Option<usize>, len: usize) {
        self.list.select(selected.filter(|index| *index < len));
        self.clamp(len);
    }

    pub fn select_first(&mut self, len: usize) {
        self.list.select((len > 0).then_some(0));
        self.clamp(len);
    }

    pub fn select_row(&mut self, visible_row: usize, len: usize) {
        self.select(self.offset().checked_add(visible_row), len);
    }

    pub fn previous(&mut self, len: usize) {
        if len == 0 {
            self.list.select(None);
            return;
        }
        let selected = self.selected().unwrap_or_default();
        self.list.select(Some(selected.checked_sub(1).unwrap_or(len - 1)));
    }

    pub fn next(&mut self, len: usize) {
        if len == 0 {
            self.list.select(None);
            return;
        }
        self.list.select(Some((self.selected().unwrap_or_default() + 1) % len));
    }

    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.list.select(None);
            *self.list.offset_mut() = 0;
        } else if self.selected().is_none_or(|selected| selected >= len) {
            self.list.select(Some(len - 1));
        }
        if self.offset() >= len {
            *self.list.offset_mut() = len.saturating_sub(1);
        }
    }

    pub fn list_state(&self) -> &ListState {
        &self.list
    }

    pub fn list_state_mut(&mut self) -> &mut ListState {
        &mut self.list
    }
}

#[cfg(test)]
mod tests {
    use super::SelectionState;

    #[test]
    fn wraps_and_handles_empty_collections() {
        let mut state = SelectionState::new(3);
        state.previous(3);
        assert_eq!(state.selected(), Some(2));
        state.next(3);
        assert_eq!(state.selected(), Some(0));
        state.select(Some(2), 3);
        state.next(3);
        assert_eq!(state.selected(), Some(0));
        state.clamp(0);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn keeps_selection_inside_visible_window() {
        let mut state = SelectionState::new(10);
        state.select(Some(7), 10);
        state.ensure_visible(10, 3);
        assert_eq!(state.offset(), 5);
        state.select(Some(2), 10);
        state.ensure_visible(10, 3);
        assert_eq!(state.offset(), 2);
    }

    #[test]
    fn visible_rows_include_list_offset() {
        let mut state = SelectionState::new(10);
        *state.list_state_mut().offset_mut() = 4;
        state.select_row(2, 10);
        assert_eq!(state.selected(), Some(6));
    }
}
