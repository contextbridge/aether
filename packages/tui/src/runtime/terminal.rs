use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io;

pub fn terminal_size() -> io::Result<(u16, u16)> {
    crossterm::terminal::size()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseCapture {
    Disabled,
    Enabled,
}

#[doc = include_str!("../docs/terminal_session.md")]
pub struct TerminalSession {
    enable_bracketed_paste: bool,
    keyboard_enhancement_enabled: bool,
}

impl TerminalSession {
    pub fn new(enable_bracketed_paste: bool, mouse_capture: MouseCapture) -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        let keyboard_enhancement_enabled =
            execute!(stdout, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)).is_ok();

        if enable_bracketed_paste {
            execute!(stdout, EnableBracketedPaste)?;
        }
        if mouse_capture == MouseCapture::Enabled {
            execute!(stdout, EnableMouseCapture)?;
        }

        Ok(Self { enable_bracketed_paste, keyboard_enhancement_enabled })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let mut stdout = io::stdout();
        // Always attempt to disable mouse capture defensively: callers may
        // toggle capture via RendererCommand after session creation, so the
        // initial `mouse_capture` field may no longer reflect terminal state.
        let _ = execute!(stdout, DisableMouseCapture);
        if self.keyboard_enhancement_enabled {
            let _ = execute!(stdout, PopKeyboardEnhancementFlags);
        }
        if self.enable_bracketed_paste {
            let _ = execute!(stdout, DisableBracketedPaste);
        }
        let _ = disable_raw_mode();
    }
}
