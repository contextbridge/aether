use ratatui::widgets::ListState;

/// Cursor and scroll offset for a list of `len` items.
///
/// `len` is supplied per call rather than stored, so one state can outlive the
/// collection it indexes: filters, reloads, and live updates all change `len`
/// without invalidating the selection.
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
        self.step(len, Direction::Backward, |_| true);
    }

    pub fn next(&mut self, len: usize) {
        self.step(len, Direction::Forward, |_| true);
    }

    /// Wrapping move to the nearest index in `direction` for which `selectable`
    /// holds. Leaves the selection untouched when no index qualifies, so panes
    /// whose rows are a mix of headers and entries never land on a header.
    pub fn step(&mut self, len: usize, direction: Direction, selectable: impl Fn(usize) -> bool) {
        if len == 0 {
            self.list.select(None);
            return;
        }
        let mut index = self.selected().unwrap_or_default().min(len - 1);
        for _ in 0..len {
            index = match direction {
                Direction::Backward => index.checked_sub(1).unwrap_or(len - 1),
                Direction::Forward => (index + 1) % len,
            };
            if selectable(index) {
                self.list.select(Some(index));
                return;
            }
        }
    }

    /// Like [`Self::step`], but stops at the ends instead of wrapping. Suits
    /// long lists that are scanned rather than cycled.
    pub fn step_clamped(&mut self, len: usize, direction: Direction, selectable: impl Fn(usize) -> bool) {
        let Some(current) = self.selected().filter(|index| *index < len) else {
            self.step(len, direction, selectable);
            return;
        };
        let next = match direction {
            Direction::Backward => (0..current).rev().find(|&index| selectable(index)),
            Direction::Forward => (current + 1..len).find(|&index| selectable(index)),
        };
        if let Some(next) = next {
            self.list.select(Some(next));
        }
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

    pub fn list_state_mut(&mut self) -> &mut ListState {
        &mut self.list
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Backward,
    Forward,
}

#[cfg(test)]
mod tests {
    use super::{Direction, SelectionState};

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

    #[test]
    fn step_skips_unselectable_rows_and_wraps() {
        let selectable = |index: usize| index % 2 == 1;
        let mut state = SelectionState::new(5);
        state.select(Some(1), 5);

        state.step(5, Direction::Forward, selectable);
        assert_eq!(state.selected(), Some(3));
        state.step(5, Direction::Forward, selectable);
        assert_eq!(state.selected(), Some(1), "wraps past the trailing unselectable row");
        state.step(5, Direction::Backward, selectable);
        assert_eq!(state.selected(), Some(3));
    }

    #[test]
    fn step_clamped_stops_at_the_ends() {
        let mut state = SelectionState::new(3);
        state.select(Some(2), 3);
        state.step_clamped(3, Direction::Forward, |_| true);
        assert_eq!(state.selected(), Some(2), "does not wrap past the last item");
        state.select(Some(0), 3);
        state.step_clamped(3, Direction::Backward, |_| true);
        assert_eq!(state.selected(), Some(0), "does not wrap past the first item");
    }

    #[test]
    fn step_leaves_selection_untouched_when_nothing_qualifies() {
        let mut state = SelectionState::new(4);
        state.select(Some(2), 4);
        state.step(4, Direction::Forward, |_| false);
        assert_eq!(state.selected(), Some(2));
    }
}
