use crossterm::event::{KeyCode, KeyEvent};

#[doc = include_str!("docs/focus_ring.md")]
pub struct FocusRing {
    focused: usize,
    len: usize,
    wrap: bool,
}

/// The result of [`FocusRing::handle_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusOutcome {
    /// Focus moved to a different index.
    FocusChanged,
    /// The key was recognized (Tab/BackTab) but focus didn't move (e.g. at boundary without wrap).
    Unchanged,
    /// The key was not a focus-navigation key and was ignored.
    Ignored,
}

impl FocusRing {
    /// Create a new `FocusRing` with wrapping enabled.
    ///
    /// Focus starts at index 0. If `len` is 0, all navigation is a no-op.
    pub fn new(len: usize) -> Self {
        Self { focused: 0, len, wrap: true }
    }

    /// Disable wrap-around: `focus_next` at the last item and `focus_prev` at
    /// the first item will not cycle.
    pub fn without_wrap(mut self) -> Self {
        self.wrap = false;
        self
    }

    /// The currently focused index.
    pub fn focused(&self) -> usize {
        self.focused
    }

    /// Returns `true` if `index` is the currently focused index.
    pub fn is_focused(&self, index: usize) -> bool {
        self.focused == index
    }

    /// The number of focusable items.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns `true` if there are no focusable items.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Update the number of focusable items. Clamps `focused` if it would be
    /// out of bounds.
    pub fn set_len(&mut self, len: usize) {
        self.len = len;
        if len == 0 {
            self.focused = 0;
        } else if self.focused >= len {
            self.focused = len - 1;
        }
    }

    /// Programmatically set focus to `index`. Returns `false` if `index` is
    /// out of bounds (focus unchanged).
    pub fn focus(&mut self, index: usize) -> bool {
        if index < self.len {
            self.focused = index;
            true
        } else {
            false
        }
    }

    /// Move focus to the next item. Returns `true` if focus changed.
    pub fn focus_next(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        if self.focused + 1 < self.len {
            self.focused += 1;
            true
        } else if self.wrap {
            self.focused = 0;
            true
        } else {
            false
        }
    }

    /// Move focus to the previous item. Returns `true` if focus changed.
    pub fn focus_prev(&mut self) -> bool {
        if self.len == 0 {
            return false;
        }
        if self.focused > 0 {
            self.focused -= 1;
            true
        } else if self.wrap {
            self.focused = self.len - 1;
            true
        } else {
            false
        }
    }

    /// Handle Tab (next) and `BackTab` (previous) key events.
    ///
    /// Returns [`FocusOutcome::FocusChanged`] if focus moved,
    /// [`FocusOutcome::Unchanged`] if a focus key was pressed but focus didn't
    /// move, or [`FocusOutcome::Ignored`] for all other keys.
    pub fn handle_key(&mut self, key_event: KeyEvent) -> FocusOutcome {
        match key_event.code {
            KeyCode::Tab => {
                if self.focus_next() {
                    FocusOutcome::FocusChanged
                } else {
                    FocusOutcome::Unchanged
                }
            }
            KeyCode::BackTab => {
                if self.focus_prev() {
                    FocusOutcome::FocusChanged
                } else {
                    FocusOutcome::Unchanged
                }
            }
            _ => FocusOutcome::Ignored,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent { code, modifiers: KeyModifiers::empty(), kind: KeyEventKind::Press, state: KeyEventState::empty() }
    }

    #[test]
    fn new_starts_at_zero() {
        let ring = FocusRing::new(3);
        assert_eq!(ring.focused(), 0);
        assert!(ring.is_focused(0));
        assert!(!ring.is_focused(1));
        assert_eq!(ring.len(), 3);
        assert!(!ring.is_empty());
    }

    #[test]
    fn cycle_forward_wraps() {
        let mut ring = FocusRing::new(3);
        assert!(ring.focus_next());
        assert_eq!(ring.focused(), 1);
        assert!(ring.focus_next());
        assert_eq!(ring.focused(), 2);
        assert!(ring.focus_next());
        assert_eq!(ring.focused(), 0); // wrapped
    }

    #[test]
    fn cycle_backward_wraps() {
        let mut ring = FocusRing::new(3);
        assert!(ring.focus_prev());
        assert_eq!(ring.focused(), 2); // wrapped to end
        assert!(ring.focus_prev());
        assert_eq!(ring.focused(), 1);
        assert!(ring.focus_prev());
        assert_eq!(ring.focused(), 0);
    }

    #[test]
    fn no_wrap_stops_at_boundaries() {
        let mut ring = FocusRing::new(3).without_wrap();

        // At start, can't go prev
        assert!(!ring.focus_prev());
        assert_eq!(ring.focused(), 0);

        // Go to end
        assert!(ring.focus_next());
        assert!(ring.focus_next());
        assert_eq!(ring.focused(), 2);

        // At end, can't go next
        assert!(!ring.focus_next());
        assert_eq!(ring.focused(), 2);
    }

    #[test]
    fn empty_ring_is_noop() {
        let mut ring = FocusRing::new(0);
        assert!(ring.is_empty());
        assert_eq!(ring.focused(), 0);
        assert!(!ring.focus_next());
        assert!(!ring.focus_prev());
        assert!(!ring.focus(0));
    }

    #[test]
    fn programmatic_focus() {
        let mut ring = FocusRing::new(5);
        assert!(ring.focus(3));
        assert_eq!(ring.focused(), 3);
        assert!(ring.is_focused(3));

        // Out of bounds
        assert!(!ring.focus(5));
        assert_eq!(ring.focused(), 3); // unchanged
    }

    #[test]
    fn set_len_clamps_focused() {
        let mut ring = FocusRing::new(5);
        ring.focus(4);
        assert_eq!(ring.focused(), 4);

        ring.set_len(3);
        assert_eq!(ring.len(), 3);
        assert_eq!(ring.focused(), 2); // clamped

        ring.set_len(0);
        assert_eq!(ring.focused(), 0);
        assert!(ring.is_empty());
    }

    #[test]
    fn set_len_preserves_focused_when_in_range() {
        let mut ring = FocusRing::new(5);
        ring.focus(2);
        ring.set_len(4);
        assert_eq!(ring.focused(), 2); // still valid
    }

    #[test]
    fn handle_key_tab_cycles_forward() {
        let mut ring = FocusRing::new(3);
        assert_eq!(ring.handle_key(key(KeyCode::Tab)), FocusOutcome::FocusChanged);
        assert_eq!(ring.focused(), 1);
    }

    #[test]
    fn handle_key_backtab_cycles_backward() {
        let mut ring = FocusRing::new(3);
        ring.focus(1);
        assert_eq!(ring.handle_key(key(KeyCode::BackTab)), FocusOutcome::FocusChanged);
        assert_eq!(ring.focused(), 0);
    }

    #[test]
    fn handle_key_other_keys_ignored() {
        let mut ring = FocusRing::new(3);
        assert_eq!(ring.handle_key(key(KeyCode::Enter)), FocusOutcome::Ignored);
        assert_eq!(ring.handle_key(key(KeyCode::Char('a'))), FocusOutcome::Ignored);
        assert_eq!(ring.focused(), 0); // unchanged
    }

    #[test]
    fn handle_key_no_wrap_returns_unchanged() {
        let mut ring = FocusRing::new(2).without_wrap();
        // At index 0, BackTab can't go further
        assert_eq!(ring.handle_key(key(KeyCode::BackTab)), FocusOutcome::Unchanged);
        assert_eq!(ring.focused(), 0);

        // Go to end
        ring.focus(1);
        assert_eq!(ring.handle_key(key(KeyCode::Tab)), FocusOutcome::Unchanged);
        assert_eq!(ring.focused(), 1);
    }

    #[test]
    fn single_item_wrap_returns_true() {
        // With wrap enabled and len=1, focus_next wraps to 0 (same index).
        // This still "changed" in the sense that the cycle completed.
        let mut ring = FocusRing::new(1);
        assert!(ring.focus_next());
        assert_eq!(ring.focused(), 0);
        assert!(ring.focus_prev());
        assert_eq!(ring.focused(), 0);
    }

    #[test]
    fn single_item_no_wrap() {
        let mut ring = FocusRing::new(1).without_wrap();
        assert!(!ring.focus_next());
        assert!(!ring.focus_prev());
        assert_eq!(ring.focused(), 0);
    }
}
