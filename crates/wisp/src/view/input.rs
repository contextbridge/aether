use crate::view::selection::Direction;
use crossterm::event::{KeyEvent, KeyModifiers};

pub enum UiEvent {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseAction, (u16, u16), KeyModifiers),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    ScrollUp,
    ScrollDown,
    Click,
    Moved,
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

impl MouseAction {
    pub fn from_event(kind: crossterm::event::MouseEventKind) -> Option<Self> {
        use crossterm::event::MouseEventKind;
        match kind {
            MouseEventKind::ScrollUp => Some(Self::ScrollUp),
            MouseEventKind::ScrollDown => Some(Self::ScrollDown),
            MouseEventKind::Down(_) => Some(Self::Click),
            MouseEventKind::Moved => Some(Self::Moved),
            _ => None,
        }
    }

    pub fn direction(self) -> Option<Direction> {
        match self {
            Self::ScrollUp => Some(Direction::Backward),
            Self::ScrollDown => Some(Direction::Forward),
            Self::Click | Self::Moved => None,
        }
    }
}
