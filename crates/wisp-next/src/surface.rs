use crate::render_context::RenderContext;
use crate::selection::Direction;
use crate::tasks::{Task, TaskResult};
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::SessionId;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use std::path::PathBuf;

/// A layer drawn over the conversation that owns all input while it is open:
/// a picker, a modal, or a full-screen view.
///
/// Exactly one is active at a time, so `App` routes keys, mouse events, and
/// rendering through this trait without re-matching on which one it is. The few
/// ACP updates that do need a concrete surface reach for it through
/// [`Layer`](crate::app::Layer) instead.
pub trait Surface {
    /// Draws the surface, returning where the terminal cursor should sit.
    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position>;

    /// Acts on whatever is focused. The default [`Surface::on_surface_key`] runs
    /// this on Enter, and list surfaces run it again when a click lands on a row.
    fn activate(&mut self) -> Vec<Action> {
        Vec::new()
    }

    /// Handles the keys unique to this surface, returning `None` to fall back to
    /// the shared navigation and filter keys in [`Surface::on_key`].
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        (key.code == KeyCode::Enter).then(|| self.activate())
    }

    /// Moves the selection, returning any work the move implies (such as
    /// fetching a preview for the newly focused row).
    fn scroll(&mut self, direction: Direction) -> Vec<Action> {
        let _ = direction;
        Vec::new()
    }

    /// Selects whatever is drawn at terminal `row`/`column`, if anything is.
    fn click(&mut self, row: u16, column: u16) -> Vec<Action> {
        let _ = (row, column);
        Vec::new()
    }

    /// The query this surface filters its list by, when it has one. Supplying
    /// it is what makes typing and backspace filter.
    fn filter(&mut self) -> Option<&mut dyn ListFilter> {
        None
    }

    /// Work implied by an edit to [`Surface::filter`], such as fetching a
    /// preview for whichever row the new query focused.
    fn on_filter_changed(&mut self) -> Vec<Action> {
        Vec::new()
    }

    /// A completed task this surface asked for.
    fn on_task_result(&mut self, result: TaskResult) -> Vec<Action> {
        let _ = result;
        Vec::new()
    }

    /// Paste content. Surfaces without an editor deliberately consume it.
    fn on_paste(&mut self, text: &str) -> Vec<Action> {
        let _ = text;
        Vec::new()
    }

    /// Releases anything the surface holds, such as an unanswered request.
    fn cancel(&mut self) {}

    fn needs_mouse_capture(&self) -> bool {
        true
    }

    /// Routes any input event through this surface's single ownership boundary.
    fn on_ui_event(&mut self, event: UiEvent) -> Vec<Action> {
        match event {
            UiEvent::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => self.on_key(key),
            UiEvent::Key(_) => Vec::new(),
            UiEvent::Paste(text) => self.on_paste(&text),
            UiEvent::Mouse(action, position) => self.on_mouse(action, position.y, position.x),
        }
    }

    /// Routes a keystroke: surface-specific keys first, then the navigation and
    /// filter keys every list surface shares.
    fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if let Some(actions) = self.on_surface_key(key) {
            return actions;
        }
        match key.code {
            KeyCode::Esc => return vec![Action::Close],
            KeyCode::Up => return self.scroll(Direction::Backward),
            KeyCode::Down => return self.scroll(Direction::Forward),
            KeyCode::Backspace => match self.filter() {
                Some(filter) => filter.pop_query_char(),
                None => return Vec::new(),
            },
            KeyCode::Char(character) if !character.is_control() => match self.filter() {
                Some(filter) => filter.push_query_char(character),
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        }
        self.on_filter_changed()
    }

    fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<Action> {
        match action {
            MouseAction::ScrollUp => self.scroll(Direction::Backward),
            MouseAction::ScrollDown => self.scroll(Direction::Forward),
            MouseAction::Click => self.click(row, column),
        }
    }
}

/// Everything a surface can ask the app to do. Flattening the per-surface
/// actions into one union lets `App` handle them in a single match, and
/// collapses several separate "close me" variants into one.
#[derive(Debug)]
pub enum Action {
    /// Dismiss the surface. Teardown (cancelling a pending elicitation, leaving
    /// workspace-move state) is the app's job, not the surface's.
    Close,
    LoadSession {
        session_id: SessionId,
        cwd: PathBuf,
    },
    RequestSessionPreview {
        session_id: String,
    },
    MoveWorkspace {
        target: WorkspaceMoveTarget,
    },
    SetConfigOption {
        config_id: String,
        value: String,
    },
    SetTheme(String),
    AuthenticateServer(String),
    AuthenticateProvider(String),
    /// Work to run off the UI thread. Its result comes back to the surface
    /// through [`Surface::on_task_result`].
    Task(Task),
    /// Send `prompt` to the agent as an ordinary turn and close the surface.
    SubmitReview(String),
}

/// The query-editing half of a filtered list, in object-safe form so surfaces
/// can hand theirs to the shared key handling.
pub trait ListFilter {
    fn push_query_char(&mut self, character: char);

    fn pop_query_char(&mut self);
}

/// Input delivered to exactly one active owner.
pub enum UiEvent {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseAction, Position),
}

/// What a mouse event asks of whatever is under the pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseAction {
    ScrollUp,
    ScrollDown,
    Click,
}

impl MouseAction {
    /// The action `kind` asks for, or nothing for the events the UI ignores.
    pub fn from_event(kind: crossterm::event::MouseEventKind) -> Option<Self> {
        use crossterm::event::MouseEventKind;
        match kind {
            MouseEventKind::ScrollUp => Some(Self::ScrollUp),
            MouseEventKind::ScrollDown => Some(Self::ScrollDown),
            MouseEventKind::Down(_) => Some(Self::Click),
            _ => None,
        }
    }
}

/// A surface's whole response, when it is at most one action.
pub fn one(action: Option<Action>) -> Vec<Action> {
    action.into_iter().collect()
}
