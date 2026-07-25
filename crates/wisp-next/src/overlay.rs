use crate::selection::Direction;
use crate::theme::Theme;
use acp_utils::notifications::WorkspaceMoveTarget;
use agent_client_protocol::schema::SessionId;
use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::path::PathBuf;

/// A layer drawn above the transcript that owns all input while it is open.
///
/// Exactly one overlay is active at a time, so `App` routes keys, mouse events,
/// and rendering through this trait without re-matching on which one it is.
pub trait Overlay {
    /// Handles a keystroke, returning whatever the app must act on.
    fn on_key(&mut self, key: KeyEvent) -> Vec<OverlayMessage>;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme);

    /// Moves the selection, returning any work the move implies (such as
    /// fetching a preview for the newly focused row).
    fn scroll(&mut self, direction: Direction) -> Vec<OverlayMessage>;

    /// Handles a click on `row`, measured from the top of `area`.
    fn click(&mut self, row: u16, area: Rect) -> Vec<OverlayMessage> {
        let _ = (row, area);
        Vec::new()
    }

    fn needs_mouse_capture(&self) -> bool {
        true
    }
}

/// Everything an overlay can ask the app to do. Flattening the per-overlay
/// message types into one union lets `App` handle them in a single match, and
/// collapses four separate "close me" variants into one.
#[derive(Debug)]
pub enum OverlayMessage {
    /// Dismiss the overlay. Teardown (cancelling a pending elicitation, leaving
    /// workspace-move state) is the app's job, not the overlay's.
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
}
