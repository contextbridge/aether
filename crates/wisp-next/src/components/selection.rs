use ratatui::layout::Rect;

/// Cursor and scroll offset for a list of `len` items.
///
/// `len` is supplied per call rather than stored, so one state can outlive the
/// collection it indexes: filters, reloads, and live updates all change `len`
/// without invalidating the selection.
///
/// Scrolling the selection into view belongs to whoever draws the rows, because
/// only it knows how many fit; [`ListView`](crate::components::list_view::ListView) writes
/// the window it settled on back through [`SelectionState::set_offset`].
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectionState {
    selected: Option<usize>,
    offset: usize,
    rows_area: Rect,
}

impl SelectionState {
    pub fn new(len: usize) -> Self {
        let mut state = Self::default();
        state.select_first(len);
        state
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Records the window the rows were actually drawn from, so the next frame
    /// scrolls against it and a click hit-tests against the same rows.
    pub fn set_offset(&mut self, offset: usize) {
        self.offset = offset;
    }

    pub fn select(&mut self, selected: Option<usize>, len: usize) {
        self.selected = selected.filter(|index| *index < len);
        self.clamp(len);
    }

    pub fn select_first(&mut self, len: usize) {
        self.selected = (len > 0).then_some(0);
        self.clamp(len);
    }

    pub fn select_row(&mut self, visible_row: usize, len: usize) {
        self.select(self.offset().checked_add(visible_row), len);
    }

    /// Records where the rows were drawn, so clicks can be mapped back to an
    /// index without every caller re-deriving the headers and borders above
    /// them. Called from rendering.
    pub fn set_rows_area(&mut self, area: Rect) {
        self.rows_area = area;
    }

    /// Where the rows were last drawn, for hit-testing a click against a pane.
    pub fn rows_area(&self) -> Rect {
        self.rows_area
    }

    /// Selects the row drawn at terminal `row`, reporting whether one was hit.
    /// Rows outside the last drawn area leave the selection alone.
    pub fn select_at(&mut self, row: u16, len: usize) -> bool {
        if row < self.rows_area.y || row >= self.rows_area.bottom() {
            return false;
        }
        self.select_row(usize::from(row - self.rows_area.y), len);
        self.selected().is_some()
    }

    /// Wrapping move to the nearest index in `direction` for which `selectable`
    /// holds. Leaves the selection untouched when no index qualifies, so panes
    /// whose rows are a mix of headers and entries never land on a header.
    pub fn step(&mut self, len: usize, direction: Direction, selectable: impl Fn(usize) -> bool) {
        if len == 0 {
            self.selected = None;
            return;
        }
        let mut index = self.selected().unwrap_or_default().min(len - 1);
        for _ in 0..len {
            index = match direction {
                Direction::Backward => index.checked_sub(1).unwrap_or(len - 1),
                Direction::Forward => (index + 1) % len,
            };
            if selectable(index) {
                self.selected = Some(index);
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
            self.selected = Some(next);
        }
    }

    pub fn clamp(&mut self, len: usize) {
        if len == 0 {
            self.selected = None;
            self.offset = 0;
        } else if self.selected().is_none_or(|selected| selected >= len) {
            self.selected = Some(len - 1);
        }
        if self.offset >= len {
            self.offset = len.saturating_sub(1);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Backward,
    Forward,
}

/// `offset` moved the least it can to bring `row` inside a viewport `height`
/// rows tall.
///
/// The panes that scroll a document rather than a [`List`](ratatui::widgets::List)
/// keep their own offset, and all of them want this same nudge.
pub fn scroll_into_view(offset: usize, row: usize, height: usize) -> usize {
    if row < offset {
        row
    } else if height > 0 && row >= offset + height {
        row + 1 - height
    } else {
        offset
    }
}

/// Shifts `value` by `amount` rows in `direction`, stopping at zero and `max`.
pub fn step_clamped(value: usize, direction: Direction, amount: usize, max: usize) -> usize {
    match direction {
        Direction::Backward => value.saturating_sub(amount),
        Direction::Forward => value.saturating_add(amount).min(max),
    }
}

#[cfg(test)]
mod tests {
    use super::{Direction, SelectionState, scroll_into_view};

    #[test]
    fn scroll_into_view_moves_the_least_it_can() {
        assert_eq!(scroll_into_view(10, 12, 5), 10, "already visible");
        assert_eq!(scroll_into_view(10, 4, 5), 4, "above the viewport");
        assert_eq!(scroll_into_view(10, 20, 5), 16, "below the viewport");
        assert_eq!(scroll_into_view(10, 14, 5), 10, "the last visible row stays put");
        assert_eq!(scroll_into_view(3, 99, 0), 3, "a zero-height viewport shows nothing to scroll to");
    }

    fn step(state: &mut SelectionState, len: usize, direction: Direction) {
        state.step(len, direction, |_| true);
    }

    #[test]
    fn wraps_and_handles_empty_collections() {
        let mut state = SelectionState::new(3);
        step(&mut state, 3, Direction::Backward);
        assert_eq!(state.selected(), Some(2));
        step(&mut state, 3, Direction::Forward);
        assert_eq!(state.selected(), Some(0));
        state.select(Some(2), 3);
        step(&mut state, 3, Direction::Forward);
        assert_eq!(state.selected(), Some(0));
        state.clamp(0);
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn visible_rows_include_list_offset() {
        let mut state = SelectionState::new(10);
        state.set_offset(4);
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
