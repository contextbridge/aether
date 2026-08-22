use crate::command::GitCommand;
use crate::view::filterable_list::FilterableList;
use crate::view::selection::Direction;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::v1::SessionId;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;

const COMPOSED_MODIFIERS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER)
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

pub(crate) fn is_composed_char(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_)) && key.modifiers.intersects(COMPOSED_MODIFIERS)
}

pub(crate) fn is_press(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

#[derive(Debug)]
pub enum SessionPickerOutput {
    Close,
    Load { session_id: SessionId, cwd: PathBuf },
    Preview(String),
}

#[derive(Debug)]
pub enum WorkspacePickerOutput {
    Close,
    Move { target: WorkspaceMoveTarget },
}

#[derive(Debug)]
pub enum SettingsOutput {
    Close,
    SetConfigOption { config_id: String, value: String },
    SetTheme(String),
    AuthenticateServer(String),
    AuthenticateProvider(String),
}

#[derive(Debug)]
pub enum ElicitationOutput {
    Close,
}

/// How a review screen ended. Both full-screen reviews report through this, so
/// the root translates every review the same way: a cancellation closes the
/// route, a submission carries the text the review produced.
#[derive(Debug)]
pub enum ReviewOutcome {
    Cancelled,
    Submitted(String),
}

#[derive(Debug)]
pub enum GitReviewOutput {
    Outcome(ReviewOutcome),
    Task(GitCommand),
}

#[derive(Debug)]
pub enum PlanReviewOutput {
    Outcome(ReviewOutcome),
}

#[derive(Debug)]
pub enum RootOutput {
    Session(SessionPickerOutput),
    Workspace(WorkspacePickerOutput),
    Settings(SettingsOutput),
    Elicitation(ElicitationOutput),
    GitReview(GitReviewOutput),
    PlanReview(PlanReviewOutput),
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

/// What a navigation event asked of the pane hosting a [`FilterableList`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Nav {
    /// The pane should close.
    Close,
    /// Enter chose the focused entry.
    Activate,
    /// A click landed on an entry and selected it.
    Clicked,
    /// The selection or filter query changed.
    Moved,
    /// Not a navigation event this list handles.
    Unhandled,
}

impl<T> FilterableList<T> {
    /// The controller every typeahead picker shares: Esc closes, Up/Down and
    /// the scroll wheel move, Enter activates, a click selects, Backspace and
    /// printable characters edit the filter query. The hosting pane matches on
    /// the outcome and adds only its own behavior.
    pub(crate) fn on_nav_event(&mut self, event: &UiEvent) -> Nav {
        match event {
            UiEvent::Key(key) if is_press(*key) => self.on_nav_key(*key),
            UiEvent::Key(_) | UiEvent::Paste(_) => Nav::Unhandled,
            UiEvent::Mouse(action, (_, row)) => match action.direction() {
                Some(direction) => {
                    self.step(direction);
                    Nav::Moved
                }
                None if self.select_at(*row) => Nav::Clicked,
                None => Nav::Unhandled,
            },
        }
    }

    fn on_nav_key(&mut self, key: KeyEvent) -> Nav {
        match key.code {
            KeyCode::Esc => Nav::Close,
            KeyCode::Enter => Nav::Activate,
            KeyCode::Up => {
                self.step(Direction::Backward);
                Nav::Moved
            }
            KeyCode::Down => {
                self.step(Direction::Forward);
                Nav::Moved
            }
            KeyCode::Backspace => {
                self.pop_query_char();
                Nav::Moved
            }
            KeyCode::Char(c) if !c.is_control() && !is_composed_char(key) => {
                self.push_query_char(c);
                Nav::Moved
            }
            _ => Nav::Unhandled,
        }
    }
}

pub(crate) fn one<T>(action: Option<T>) -> Vec<T> {
    action.into_iter().collect()
}
