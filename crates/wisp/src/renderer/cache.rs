use std::collections::HashMap;
use std::rc::Rc;

use crate::conversation::{ConversationItemId, Revision};
use crate::view::generation::Generation;
use ratatui::text::Line;

/// The inputs a sealed item's rendering depends on, besides its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RenderShape {
    pub(super) width: u16,
    pub(super) padding: u16,
    pub(super) theme: Generation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct RenderKey {
    pub(super) item_id: ConversationItemId,
    pub(super) revision: Revision,
    pub(super) shape: RenderShape,
    /// The spinner frame an open tool call is rendered with; `None` for
    /// anything whose rendering the spinner cannot move.
    pub(super) spinner: Option<usize>,
}

/// Two generations of memoized item rows: `frame` is being filled by the
/// current draw, `current` still holds the previous draw's entries so a key
/// that misses `frame` can be carried over instead of rebuilt. At the end of a
/// draw the surviving `frame` becomes `current` and everything else is
/// dropped.
#[derive(Default)]
pub(super) struct RenderCache {
    pub(super) current: HashMap<RenderKey, Rc<[Line<'static>]>>,
    pub(super) frame: HashMap<RenderKey, Rc<[Line<'static>]>>,
}

impl RenderCache {
    /// Returns the cached rows for `key`, building them with `build` on a
    /// miss, and whether they were built.
    pub(super) fn get_or_insert_with(
        &mut self,
        key: RenderKey,
        build: impl FnOnce() -> Rc<[Line<'static>]>,
    ) -> (Rc<[Line<'static>]>, bool) {
        if let Some(lines) = self.frame.remove(&key).or_else(|| self.current.remove(&key)) {
            self.frame.insert(key, Rc::clone(&lines));
            return (lines, false);
        }
        let lines = build();
        self.frame.insert(key, Rc::clone(&lines));
        (lines, true)
    }

    pub(super) fn clear(&mut self) {
        self.current.clear();
        self.frame.clear();
    }
}
