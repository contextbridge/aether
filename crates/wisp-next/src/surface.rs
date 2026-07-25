use crate::effects::{Effect, EffectResult};
use crate::render_context::RenderContext;
use crate::screens::MouseAction;
use crate::selection::Direction;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::SessionId;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use std::any::Any;
use std::path::PathBuf;

/// A layer drawn over the conversation that owns all input while it is open:
/// a picker, a modal, or a full-screen view.
///
/// Exactly one is active at a time, so `App` routes keys, mouse events, and
/// rendering through this trait without re-matching on which one it is. The
/// `Any` bound is what lets the handful of ACP updates that need a concrete
/// surface find it, without the app tracking which kind is open.
pub trait Surface: Any {
    /// Handles the keys unique to this surface, returning `None` to fall back to
    /// the shared navigation and filter keys in [`Surface::on_key`].
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<SurfaceMessage>>;

    /// Draws the surface, returning where the terminal cursor should sit.
    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut RenderContext<'_>) -> Option<Position>;

    /// Whether this surface replaces the conversation rather than drawing above
    /// it, which also stops the transcript committing to terminal scrollback.
    fn is_fullscreen(&self) -> bool {
        false
    }

    /// Moves the selection, returning any work the move implies (such as
    /// fetching a preview for the newly focused row).
    fn scroll(&mut self, direction: Direction) -> Vec<SurfaceMessage> {
        let _ = direction;
        Vec::new()
    }

    /// Selects whatever is drawn at terminal `row`/`column`, if anything is.
    fn click(&mut self, row: u16, column: u16) -> Vec<SurfaceMessage> {
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
    fn on_filter_changed(&mut self) -> Vec<SurfaceMessage> {
        Vec::new()
    }

    /// The result of a [`SurfaceMessage::Effect`] this surface asked for.
    fn on_event(&mut self, event: EffectResult) -> Vec<SurfaceMessage> {
        let _ = event;
        Vec::new()
    }

    /// Releases anything the surface holds, such as an unanswered request.
    fn cancel(&mut self) {}

    fn needs_mouse_capture(&self) -> bool {
        true
    }

    /// Routes a keystroke: surface-specific keys first, then the navigation and
    /// filter keys every list surface shares.
    fn on_key(&mut self, key: KeyEvent) -> Vec<SurfaceMessage> {
        if let Some(messages) = self.on_surface_key(key) {
            return messages;
        }
        match key.code {
            KeyCode::Esc => return vec![SurfaceMessage::Close],
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

    fn on_mouse(&mut self, action: MouseAction, row: u16, column: u16) -> Vec<SurfaceMessage> {
        match action {
            MouseAction::ScrollUp => self.scroll(Direction::Backward),
            MouseAction::ScrollDown => self.scroll(Direction::Forward),
            MouseAction::Click => self.click(row, column),
        }
    }
}

/// Everything a surface can ask the app to do. Flattening the per-surface
/// message types into one union lets `App` handle them in a single match, and
/// collapses several separate "close me" variants into one.
#[derive(Debug)]
pub enum SurfaceMessage {
    /// Dismiss the surface. Teardown (cancelling a pending elicitation, leaving
    /// workspace-move state) is the app's job, not the surface's.
    Close,
    /// Return to the surface that opened this one. Handled by that surface, so
    /// it never reaches the app.
    Back,
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
    /// through [`Surface::on_event`].
    Effect(Effect),
    /// Send `prompt` to the agent as an ordinary turn and close the surface.
    SubmitReview(String),
}

/// The query-editing half of a filtered list, in object-safe form so surfaces
/// can hand theirs to the shared key handling.
pub trait ListFilter {
    fn push_query_char(&mut self, character: char);

    fn pop_query_char(&mut self);
}
