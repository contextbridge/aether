#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct VerticalCursor {
    pub row: usize,
    pub scroll: usize,
}

impl VerticalCursor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn move_by(&mut self, delta: isize, max: usize) -> bool {
        let next = if delta.is_negative() {
            self.row.saturating_sub(delta.unsigned_abs())
        } else {
            (self.row + delta.unsigned_abs()).min(max)
        };
        let changed = next != self.row;
        self.row = next;
        changed
    }

    pub fn move_to_start(&mut self) -> bool {
        let changed = self.row != 0;
        self.row = 0;
        changed
    }

    pub fn move_to_end(&mut self, max: usize) -> bool {
        let changed = self.row != max;
        self.row = max;
        changed
    }

    pub fn ensure_visible(&mut self, visual_row: usize, viewport_height: usize) {
        if viewport_height == 0 {
            return;
        }

        if visual_row < self.scroll {
            self.scroll = visual_row;
        } else if visual_row >= self.scroll + viewport_height {
            self.scroll = visual_row.saturating_sub(viewport_height - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_by_clamps_to_max() {
        let mut cursor = VerticalCursor::new();
        assert!(cursor.move_by(5, 3));
        assert_eq!(cursor.row, 3);
        assert!(!cursor.move_by(5, 3));
    }

    #[test]
    fn move_by_handles_negative_delta() {
        let mut cursor = VerticalCursor { row: 2, scroll: 0 };
        assert!(cursor.move_by(-10, 5));
        assert_eq!(cursor.row, 0);
    }

    #[test]
    fn ensure_visible_scrolls_into_view() {
        let mut cursor = VerticalCursor { row: 0, scroll: 0 };
        cursor.ensure_visible(10, 5);
        assert_eq!(cursor.scroll, 6);

        cursor.ensure_visible(2, 5);
        assert_eq!(cursor.scroll, 2);
    }
}
