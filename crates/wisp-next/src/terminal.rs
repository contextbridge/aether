//! The terminal state wisp-next drives itself: the modes it switches on beyond
//! the ones ratatui manages, and the inline viewport it draws into.

use crossterm::cursor::MoveTo;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::{DefaultTerminal, Terminal, TerminalOptions, Viewport};
use std::io::{self, Stdout};

/// The terminal modes the UI turns on for itself, restored when this is dropped.
///
/// Ratatui's panic hook only knows about raw mode and the alternate screen; it
/// cannot know the app also asked for bracketed paste, keyboard enhancement
/// flags, or mouse reporting. Owning them here is what hands the terminal back
/// intact when the event loop unwinds rather than returns.
pub struct TerminalModes {
    out: Stdout,
    keyboard_enhancement: bool,
    mouse_capture: bool,
}

/// Rows left above the inline viewport for the scrollback the UI commits into
/// the terminal's own history.
pub const INLINE_SCROLLBACK_RESERVE: u16 = 2;

pub fn inline_viewport_height(terminal_height: u16) -> u16 {
    if terminal_height == 0 { 0 } else { terminal_height.saturating_sub(INLINE_SCROLLBACK_RESERVE).max(1) }
}

/// Rebuilds `terminal` when the window no longer matches the inline viewport.
///
/// Ratatui keeps the height an inline viewport was created with: it clamps that
/// height to a window that shrank but never grows it back, and a clamped
/// viewport swallows the rows this UI commits its scrollback through. Only a
/// fresh terminal puts the two back in step.
pub fn resync_inline_viewport(terminal: &mut DefaultTerminal) -> Result<(), io::Error> {
    let terminal_height = terminal.size()?.height;
    let height = inline_viewport_height(terminal_height);
    if height == 0 || height == terminal.get_frame().area().height {
        return Ok(());
    }

    // The viewport takes everything below the reserve, and the fresh terminal
    // starts from an empty buffer that would not know to erase what the old one
    // left painted there.
    let mut out = io::stdout();
    execute!(out, MoveTo(0, terminal_height - height), Clear(ClearType::FromCursorDown))?;
    *terminal =
        Terminal::with_options(CrosstermBackend::new(out), TerminalOptions { viewport: Viewport::Inline(height) })?;
    Ok(())
}

impl TerminalModes {
    /// Switches on the modes the UI needs for its whole run.
    pub fn enable() -> Result<Self, io::Error> {
        let mut modes = Self { out: io::stdout(), keyboard_enhancement: false, mouse_capture: false };
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        modes.keyboard_enhancement = execute!(modes.out, PushKeyboardEnhancementFlags(flags)).is_ok();
        execute!(modes.out, EnableBracketedPaste)?;
        Ok(modes)
    }

    /// Turns mouse reporting on only while something on screen wants it, so an
    /// ordinary composer leaves the terminal's own selection and scrollback
    /// alone.
    pub fn set_mouse_capture(&mut self, enabled: bool) {
        if enabled == self.mouse_capture {
            return;
        }
        let applied =
            if enabled { execute!(self.out, EnableMouseCapture) } else { execute!(self.out, DisableMouseCapture) };
        if applied.is_ok() {
            self.mouse_capture = enabled;
        }
    }
}

impl Drop for TerminalModes {
    fn drop(&mut self) {
        self.set_mouse_capture(false);
        let _ = execute!(self.out, DisableBracketedPaste);
        if self.keyboard_enhancement {
            let _ = execute!(self.out, PopKeyboardEnhancementFlags);
        }
    }
}
