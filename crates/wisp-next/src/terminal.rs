//! The terminal session the UI runs in: ratatui's raw mode / inline viewport
//! plus the crossterm mode escapes layered on top, owned by one RAII guard.

use crossterm::cursor::MoveTo;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::{DefaultTerminal, Terminal, TerminalOptions, Viewport};
use std::io;
use std::thread;

/// Rows left above the inline viewport for the scrollback the UI commits into
/// the terminal's own history.
pub const INLINE_SCROLLBACK_RESERVE: u16 = 2;

pub fn inline_viewport_height(terminal_height: u16) -> u16 {
    if terminal_height == 0 { 0 } else { terminal_height.saturating_sub(INLINE_SCROLLBACK_RESERVE).max(1) }
}

/// Whether the inline viewport no longer matches the window it was created for.
///
/// Ratatui keeps the height an inline viewport was created with: it clamps that
/// height to a window that shrank but never grows it back, so a stale viewport
/// both wastes rows after a shrink and swallows the rows this UI commits its
/// scrollback through after a regrow. Only a rebuilt terminal puts the two back
/// in step.
pub fn inline_viewport_needs_resync(terminal_height: u16, current_viewport_height: u16) -> bool {
    let height = inline_viewport_height(terminal_height);
    height != 0 && height != current_viewport_height
}

/// Owns the terminal for the whole UI run: ratatui's raw mode / inline viewport
/// and the crossterm mode escapes on top, undone when this is dropped.
/// Construct only through [`TerminalSession::enter`].
pub(crate) struct TerminalSession {
    terminal: DefaultTerminal,
    keyboard_enhancement: bool,
    bracketed_paste: bool,
    mouse_capture: bool,
}

impl TerminalSession {
    /// Enters ratatui (raw mode, inline viewport, global panic hook) and
    /// switches on the modes the UI needs for its whole run. Keyboard
    /// enhancement is best-effort; bracketed paste is required. On any failure
    /// the terminal is restored best-effort before the original error is
    /// returned.
    pub(crate) fn enter(viewport: Viewport) -> io::Result<Self> {
        let terminal = match ratatui::try_init_with_options(TerminalOptions { viewport }) {
            Ok(terminal) => terminal,
            Err(error) => {
                // Raw mode may already be active; restore best-effort and keep
                // the original init error authoritative.
                ratatui::restore();
                return Err(error);
            }
        };
        let mut session = Self { terminal, keyboard_enhancement: false, bracketed_paste: false, mouse_capture: false };
        session.try_keyboard_enhancement();
        // A failure here drops the session, which undoes the modes that landed
        // and restores ratatui before the error propagates.
        session.apply_bracketed_paste()?;
        Ok(session)
    }

    pub(crate) fn terminal_mut(&mut self) -> &mut DefaultTerminal {
        &mut self.terminal
    }

    /// Turns mouse reporting on only while something on screen wants it, so an
    /// ordinary composer leaves the terminal's own selection and scrollback alone.
    pub(crate) fn set_mouse_capture(&mut self, enabled: bool) {
        if enabled == self.mouse_capture {
            return;
        }
        if enabled {
            // Enable is best-effort; if it fails, roll back best-effort and keep
            // the flag on only when that rollback also failed (so teardown retries).
            let enabled_ok = execute!(io::stdout(), EnableMouseCapture).is_ok();
            self.mouse_capture = enabled_ok || execute!(io::stdout(), DisableMouseCapture).is_err();
        } else if execute!(io::stdout(), DisableMouseCapture).is_ok() {
            self.mouse_capture = false;
        }
    }

    /// Rebuilds the terminal when the window no longer matches the inline
    /// viewport it was created with: after a shrink (ratatui clamps the viewport
    /// height) or a regrow (it never grows the height back).
    pub(crate) fn resync_inline_viewport(&mut self) -> io::Result<()> {
        let terminal_height = self.terminal.size()?.height;
        let height = inline_viewport_height(terminal_height);
        if !inline_viewport_needs_resync(terminal_height, self.terminal.get_frame().area().height) {
            return Ok(());
        }

        execute!(io::stdout(), MoveTo(0, terminal_height - height), Clear(ClearType::FromCursorDown))?;
        self.terminal = Terminal::with_options(
            CrosstermBackend::new(io::stdout()),
            TerminalOptions { viewport: Viewport::Inline(height) },
        )?;
        Ok(())
    }

    fn try_keyboard_enhancement(&mut self) {
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        if execute!(io::stdout(), PushKeyboardEnhancementFlags(flags)).is_ok() {
            self.keyboard_enhancement = true;
        } else if execute!(io::stdout(), PopKeyboardEnhancementFlags).is_err() {
            // A failed push can still have reached the terminal, and the pop that
            // should have undone it failed too, so teardown retries.
            self.keyboard_enhancement = true;
        }
    }

    fn apply_bracketed_paste(&mut self) -> io::Result<()> {
        match execute!(io::stdout(), EnableBracketedPaste) {
            Ok(()) => {
                self.bracketed_paste = true;
                Ok(())
            }
            Err(error) => {
                // A failed enable can still have reached the terminal.
                if execute!(io::stdout(), DisableBracketedPaste).is_err() {
                    self.bracketed_paste = true;
                }
                Err(error)
            }
        }
    }

    fn undo_modes(&mut self) {
        if self.mouse_capture {
            let _ = execute!(io::stdout(), DisableMouseCapture);
            self.mouse_capture = false;
        }
        if self.bracketed_paste {
            let _ = execute!(io::stdout(), DisableBracketedPaste);
            self.bracketed_paste = false;
        }
        if self.keyboard_enhancement {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
            self.keyboard_enhancement = false;
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        // Reverse order: the UI's escapes are torn down while raw mode is still
        // on, then ratatui (raw mode off) is restored.
        self.undo_modes();
        // On a panic, ratatui's global panic hook already restored before
        // unwinding reached this `Drop`; skip our own restore so ratatui is
        // restored exactly once instead of twice.
        if !thread::panicking() {
            ratatui::restore();
        }
    }
}
