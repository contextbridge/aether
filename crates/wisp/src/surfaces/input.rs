use crate::command::GitReviewCommand;
use crate::settings::overlay::SettingsChange;
use crate::view::filterable_list::FilterableList;
use crate::view::selection::{Direction, SelectionState};
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::v2::SessionId;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

const COMPOSED_MODIFIERS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER)
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

pub(crate) fn is_composed_char(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_)) && key.modifiers.intersects(COMPOSED_MODIFIERS)
}

/// What a surface asks of the app. Every overlay, route and pane reports
/// through this one enum, so the app acts on each request in one place.
#[derive(Debug)]
pub enum UiOutput {
    Close,
    ConfigChange(SettingsChange),
    SetTheme(String),
    AuthenticateServer(String),
    AuthenticateProvider(String),
    ResumeSession { session_id: SessionId, cwd: PathBuf },
    PreviewSession(String),
    MoveWorkspace(WorkspaceMoveTarget),
    ArtifactApproved,
    ArtifactFeedback(String),
    SubmitReview(String),
    GitReview(GitReviewCommand),
}

pub enum UiEvent {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseAction, (u16, u16)),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    ScrollUp,
    ScrollDown,
    Click,
}

impl MouseAction {
    pub fn from_event(kind: crossterm::event::MouseEventKind) -> Option<Self> {
        use crossterm::event::MouseEventKind;
        match kind {
            MouseEventKind::ScrollUp => Some(Self::ScrollUp),
            MouseEventKind::ScrollDown => Some(Self::ScrollDown),
            MouseEventKind::Down(_) => Some(Self::Click),
            _ => None,
        }
    }

    /// The list direction a scroll notch maps to; a click maps to none.
    pub fn direction(self) -> Option<Direction> {
        match self {
            Self::ScrollUp => Some(Direction::Backward),
            Self::ScrollDown => Some(Direction::Forward),
            Self::Click => None,
        }
    }
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

impl SelectionState {
    pub(crate) fn on_nav_event(&mut self, event: &UiEvent, len: usize, selectable: impl Fn(usize) -> bool) -> Nav {
        let direction = match event {
            UiEvent::Key(key) => match key.code {
                KeyCode::Esc => return Nav::Close,
                KeyCode::Enter => return Nav::Activate,
                KeyCode::Up => Direction::Backward,
                KeyCode::Down => Direction::Forward,
                _ => return Nav::Unhandled,
            },
            UiEvent::Paste(_) => return Nav::Unhandled,
            UiEvent::Mouse(action, (_, row)) => match action.direction() {
                Some(direction) => direction,
                None if self.select_at(*row, len, selectable) => return Nav::Clicked,
                None => return Nav::Unhandled,
            },
        };
        self.step(len, direction, selectable);
        Nav::Moved
    }
}

impl<T> FilterableList<T> {
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
        let (selection, len, selectable) = self.navigation();
        selection.on_nav_event(event, len, selectable)
    }
}

pub(crate) fn one<T>(action: Option<T>) -> Vec<T> {
    action.into_iter().collect()
}
