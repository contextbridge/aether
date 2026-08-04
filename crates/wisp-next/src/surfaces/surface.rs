use crate::components::selection::Direction;
use crate::session::tasks::{Task, TaskResult};
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::SessionId;
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::path::PathBuf;

pub(crate) const COMPOSED_MODIFIERS: KeyModifiers = KeyModifiers::CONTROL
    .union(KeyModifiers::ALT)
    .union(KeyModifiers::SUPER)
    .union(KeyModifiers::HYPER)
    .union(KeyModifiers::META);

pub(crate) fn is_composed_char(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char(_)) && key.modifiers.intersects(COMPOSED_MODIFIERS)
}

/// Whether `key` is a keystroke rather than the release half of one. Surfaces
/// that route [`UiEvent`]s themselves apply this filter in place of the one
/// [`Surface::on_ui_event`] does for them.
pub(crate) fn is_press(key: KeyEvent) -> bool {
    matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat)
}

/// A layer drawn over the conversation that owns all input while it is open:
/// a picker, a modal, or a full-screen view.
///
/// Exactly one is active at a time, so `App` routes keys, mouse events, and
/// lifecycle work through this trait. Rendering is dispatched separately by
/// the closed [`Layer`](crate::app::Layer) enum.
pub trait Surface {
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

    /// The filtered list this surface is a view of, when it is one.
    ///
    /// Supplying it is what makes typing filter, arrows and the wheel navigate,
    /// and a click select — a surface that has one writes none of that itself.
    fn list(&mut self) -> Option<&mut dyn SurfaceList> {
        None
    }

    /// Work implied by the selection moving, however it moved: a filter edit, an
    /// arrow, a wheel notch, or a click. Fetching a preview for the newly
    /// focused row goes here.
    fn on_selection_changed(&mut self) -> Vec<Action> {
        Vec::new()
    }

    /// Whether a click acts on the row it lands on as well as focusing it.
    /// Pickers whose rows are cheap to act on say yes; those whose rows commit
    /// the whole session only move focus.
    fn activates_on_click(&self) -> bool {
        false
    }

    /// Moves the selection, returning any work the move implies.
    fn scroll(&mut self, direction: Direction) -> Vec<Action> {
        let Some(list) = self.list() else {
            return Vec::new();
        };
        list.step(direction);
        self.on_selection_changed()
    }

    /// Selects whatever is drawn at terminal `row`/`column`, if anything is.
    fn click(&mut self, row: u16, column: u16) -> Vec<Action> {
        let _ = column;
        let Some(list) = self.list() else {
            return Vec::new();
        };
        if !list.select_at(row) {
            return Vec::new();
        }
        let mut actions = self.on_selection_changed();
        if self.activates_on_click() {
            actions.extend(self.activate());
        }
        actions
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
            UiEvent::Key(key) if is_press(key) => self.on_key(key),
            UiEvent::Key(_) => Vec::new(),
            UiEvent::Paste(text) => self.on_paste(&text),
            UiEvent::Mouse(action, (column, row)) => self.on_mouse(action, row, column),
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
            KeyCode::Backspace => match self.list() {
                Some(list) => list.pop_query_char(),
                None => return Vec::new(),
            },
            KeyCode::Char(character) if !character.is_control() && !is_composed_char(key) => match self.list() {
                Some(list) => list.push_query_char(character),
                None => return Vec::new(),
            },
            _ => return Vec::new(),
        }
        self.on_selection_changed()
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

/// The navigable half of a filtered list, in object-safe form so surfaces can
/// hand theirs to the shared key and mouse handling without it knowing the item
/// type.
pub trait SurfaceList {
    fn push_query_char(&mut self, character: char);

    fn pop_query_char(&mut self);

    /// Moves the selection one entry in `direction`, under whatever navigation
    /// policy the list was built with.
    fn step(&mut self, direction: Direction);

    /// Selects the entry drawn at terminal `row`, reporting whether one was hit.
    fn select_at(&mut self, row: u16) -> bool;
}

/// Input delivered to exactly one active owner.
pub enum UiEvent {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseAction, (u16, u16)),
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
