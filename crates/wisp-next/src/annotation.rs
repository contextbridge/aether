//! Comments a reviewer attaches to a line of a document, and the one being
//! typed.
//!
//! Both review screens — the git diff and the plan review — anchor comments to
//! a line and edit them the same way, so the editing half lives here and each
//! screen only decides how its own box is drawn.

use crate::edit_buffer::{EditBuffer, apply_edit_key};
use crossterm::event::{KeyCode, KeyEvent};

/// A comment being typed, anchored to wherever `A` points.
pub struct Draft<A> {
    pub anchor: A,
    pub buffer: EditBuffer,
}

/// What a keystroke did to a [`Draft`].
pub enum DraftOutcome {
    /// Still being typed.
    Continue,
    /// Abandoned, or finished empty. Either way there is nothing to file.
    Discard,
    /// Finished, with the body that was typed.
    Commit(String),
}

impl<A> Draft<A> {
    pub fn new(anchor: A) -> Self {
        Self { anchor, buffer: EditBuffer::default() }
    }

    pub fn on_paste(&mut self, text: &str) {
        self.buffer.insert_paste(text);
    }

    /// Applies one keystroke. Enter files whatever was typed, Esc abandons it,
    /// and everything else goes to the shared editing keys.
    pub fn on_key(&mut self, key: KeyEvent) -> DraftOutcome {
        match key.code {
            KeyCode::Esc => DraftOutcome::Discard,
            KeyCode::Enter => {
                let body = self.buffer.take();
                if body.trim().is_empty() { DraftOutcome::Discard } else { DraftOutcome::Commit(body) }
            }
            _ => {
                apply_edit_key(&mut self.buffer, key);
                DraftOutcome::Continue
            }
        }
    }
}
