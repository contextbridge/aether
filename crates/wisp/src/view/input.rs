use crate::view::selection::Direction;
use crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};

pub enum UiEvent {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
}

/// What a navigation event asked of the pane hosting a list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Nav {
    /// The pane should close.
    Close,
    /// Enter chose the focused entry.
    Activate,
    /// A click moved the selection to an entry, which may also activate it.
    Clicked,
    /// The selection or filter query changed.
    Moved,
    /// Not a navigation event this list handles.
    Unhandled,
}

pub fn scroll_direction(kind: MouseEventKind) -> Option<Direction> {
    match kind {
        MouseEventKind::ScrollUp => Some(Direction::Backward),
        MouseEventKind::ScrollDown => Some(Direction::Forward),
        _ => None,
    }
}
