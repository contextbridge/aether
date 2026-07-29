//! Terminal modes the UI drives beyond ratatui's raw mode / alternate screen,
//! and the inline viewport it draws into.

use crossterm::cursor::MoveTo;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{Clear, ClearType};
use ratatui::backend::CrosstermBackend;
use ratatui::{DefaultTerminal, Terminal, TerminalOptions, Viewport};
use std::future::Future;
use std::io::{self, Stdout};
use std::thread;

/// Low-level terminal operations the TUI lifecycle drives: ratatui init/restore
/// and the crossterm mode escapes layered on top of ratatui's raw mode. Injectable
/// so the whole init -> setup -> body -> teardown lifecycle is exercised without
/// a real TTY.
///
/// An enable that returns `Err` may still have written part of its escape
/// sequence, so a failed enable is always paired with a best-effort disable.
/// Disable/restore calls are best-effort: one stuck mode never strands the rest
/// of teardown.
pub trait TerminalIo {
    /// The ratatui terminal this backend produces. `()` in tests; the real
    /// [`DefaultTerminal`] in production.
    type Terminal;

    /// Equivalent to [`ratatui::try_init_with_options`]: installs ratatui's
    /// global panic hook, enables raw mode, and builds the terminal.
    ///
    /// On `Err`, raw mode may already be active (raw mode succeeded but terminal
    /// construction failed); [`run_terminal_lifecycle`] restores best-effort
    /// before propagating so the terminal is never left stranded in raw mode.
    fn init_ratatui(&mut self, options: TerminalOptions) -> io::Result<Self::Terminal>;

    /// Best-effort ratatui restore (raw mode off, alternate screen off). Idempotent.
    fn restore_ratatui(&mut self);

    fn push_keyboard_enhancement(&mut self, flags: KeyboardEnhancementFlags) -> io::Result<()>;
    fn pop_keyboard_enhancement(&mut self) -> io::Result<()>;
    fn enable_bracketed_paste(&mut self) -> io::Result<()>;
    fn disable_bracketed_paste(&mut self) -> io::Result<()>;
    fn enable_mouse_capture(&mut self) -> io::Result<()>;
    fn disable_mouse_capture(&mut self) -> io::Result<()>;
}

/// Where in [`run_terminal_lifecycle`] a failure originated, so the caller keeps
/// the original typed error and can tell a setup failure (ratatui already
/// restored) from a body failure.
#[derive(Debug)]
pub enum LifecycleError<E> {
    /// [`TerminalIo::init_ratatui`] failed. The lifecycle already restored ratatui
    /// best-effort; no extra modes were enabled.
    Init(io::Error),
    /// [`TerminalModes::enable`] failed. `enable` already tore its partial modes
    /// down and restored ratatui.
    Setup(io::Error),
    /// `body` returned an error. Teardown still ran (best-effort) before this
    /// propagated, so the terminal is restored; the original error is preserved.
    Runtime(E),
}

/// Runs the full TUI terminal lifecycle — ratatui init, extra-mode setup, `body`,
/// then teardown — restoring the terminal on every return path.
///
/// `body` owns the terminal and the mode guard for its whole duration, so their
/// teardown runs when it finishes — on `Ok`, `Err`, or a panic unwinding out of
/// `body` — making `body`'s result always authoritative.
///
/// Cleanup on each path:
/// - **Init fails**: ratatui is restored best-effort (raw mode may be on) and the
///   original error is returned as [`LifecycleError::Init`].
/// - **Mode setup fails**: [`TerminalModes::enable`] undoes whatever landed and
///   restores ratatui, then returns [`LifecycleError::Setup`].
/// - **`body` returns `Ok`/`Err`**: the guard's `Drop` undoes the extra modes
///   while raw mode is still on, then restores ratatui; the body's result is
///   returned untouched (wrapped as [`LifecycleError::Runtime`] on `Err`).
/// - **`body` panics**: ratatui's global panic hook (installed by
///   [`TerminalIo::init_ratatui`]) restores ratatui before unwinding reaches the
///   guard; the guard's `Drop` undoes the extra modes and skips its own restore
///   (it can see the panic in flight), so ratatui is restored exactly once.
pub async fn run_terminal_lifecycle<I, F, Fut, T, E>(
    io: I,
    options: TerminalOptions,
    body: F,
) -> Result<T, LifecycleError<E>>
where
    I: TerminalIo,
    F: FnOnce(I::Terminal, TerminalModes<I>) -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let mut io = io;
    let terminal = match io.init_ratatui(options) {
        Ok(terminal) => terminal,
        Err(error) => {
            // Raw mode may already be active (enable_raw_mode succeeded but
            // terminal construction failed). Restore best-effort; the original
            // init error stays authoritative.
            io.restore_ratatui();
            return Err(LifecycleError::Init(error));
        }
    };
    let modes = TerminalModes::enable(io).map_err(LifecycleError::Setup)?;
    body(terminal, modes).await.map_err(LifecycleError::Runtime)
}

/// Modes the UI turns on beyond the raw mode / alternate screen ratatui owns,
/// undone when this is dropped.
///
/// Construct only after ratatui is initialized (which also installs ratatui's
/// global panic hook). `Drop` undoes each enabled mode while raw mode is still
/// on, then restores ratatui. On a panic unwinding out of the body, ratatui's
/// global panic hook restores ratatui before this guard's `Drop` runs; `Drop`
/// detects the in-flight panic and skips its own restore, so ratatui is restored
/// exactly once on every path rather than twice.
pub struct TerminalModes<I: TerminalIo> {
    io: I,
    keyboard_enhancement: bool,
    bracketed_paste: bool,
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

    let mut out = io::stdout();
    execute!(out, MoveTo(0, terminal_height - height), Clear(ClearType::FromCursorDown))?;
    *terminal =
        Terminal::with_options(CrosstermBackend::new(out), TerminalOptions { viewport: Viewport::Inline(height) })?;
    Ok(())
}

/// [`TerminalIo`] backed by the real stdout, crossterm, and ratatui.
pub(crate) struct StdTerminalIo {
    out: Stdout,
}

impl StdTerminalIo {
    pub(crate) fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl Default for StdTerminalIo {
    fn default() -> Self {
        Self::new()
    }
}

impl TerminalIo for StdTerminalIo {
    type Terminal = DefaultTerminal;

    fn init_ratatui(&mut self, options: TerminalOptions) -> io::Result<DefaultTerminal> {
        ratatui::try_init_with_options(options)
    }

    fn restore_ratatui(&mut self) {
        ratatui::restore();
    }

    fn push_keyboard_enhancement(&mut self, flags: KeyboardEnhancementFlags) -> io::Result<()> {
        execute!(self.out, PushKeyboardEnhancementFlags(flags))
    }

    fn pop_keyboard_enhancement(&mut self) -> io::Result<()> {
        execute!(self.out, PopKeyboardEnhancementFlags)
    }

    fn enable_bracketed_paste(&mut self) -> io::Result<()> {
        execute!(self.out, EnableBracketedPaste)
    }

    fn disable_bracketed_paste(&mut self) -> io::Result<()> {
        execute!(self.out, DisableBracketedPaste)
    }

    fn enable_mouse_capture(&mut self) -> io::Result<()> {
        execute!(self.out, EnableMouseCapture)
    }

    fn disable_mouse_capture(&mut self) -> io::Result<()> {
        execute!(self.out, DisableMouseCapture)
    }
}

impl<I: TerminalIo> TerminalModes<I> {
    /// Switches on the modes the UI needs for its whole run. The caller must
    /// already have entered ratatui.
    ///
    /// Keyboard enhancement is best-effort; bracketed paste is required. A failed
    /// enable may still have written part of its escape sequence, so each failed
    /// enable is paired with a best-effort disable before the original error is
    /// returned; `Drop` then undoes anything that landed and restores ratatui.
    pub fn enable(io: I) -> Result<Self, io::Error> {
        let mut modes = Self { io, keyboard_enhancement: false, bracketed_paste: false, mouse_capture: false };
        let flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
            | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES;
        modes.keyboard_enhancement = modes.try_keyboard_enhancement(flags);
        modes.apply_bracketed_paste()?;
        Ok(modes)
    }

    /// Turns mouse reporting on only while something on screen wants it, so an
    /// ordinary composer leaves the terminal's own selection and scrollback alone.
    ///
    /// A failed enable may have partially landed, so it is rolled back
    /// best-effort; if that rollback also fails the flag stays on so `Drop`
    /// retries. A failed disable likewise leaves the flag on for a `Drop` retry.
    pub fn set_mouse_capture(&mut self, enabled: bool) {
        if enabled == self.mouse_capture {
            return;
        }
        if enabled {
            // Enable is best-effort; if it fails, roll back best-effort and keep
            // the flag on only when that rollback also failed (so teardown retries).
            let enabled_ok = self.io.enable_mouse_capture().is_ok();
            self.mouse_capture = enabled_ok || self.io.disable_mouse_capture().is_err();
        } else if self.io.disable_mouse_capture().is_ok() {
            self.mouse_capture = false;
        }
    }

    fn try_keyboard_enhancement(&mut self, flags: KeyboardEnhancementFlags) -> bool {
        if self.io.push_keyboard_enhancement(flags).is_ok() {
            true
        } else {
            // A failed push can still have reached the terminal.
            if self.io.pop_keyboard_enhancement().is_err() {
                self.keyboard_enhancement = true;
            }
            false
        }
    }

    fn apply_bracketed_paste(&mut self) -> io::Result<()> {
        match self.io.enable_bracketed_paste() {
            Ok(()) => {
                self.bracketed_paste = true;
                Ok(())
            }
            Err(error) => {
                // A failed enable can still have reached the terminal.
                if self.io.disable_bracketed_paste().is_err() {
                    self.bracketed_paste = true;
                }
                Err(error)
            }
        }
    }

    fn undo_modes(&mut self) {
        if self.mouse_capture {
            let _ = self.io.disable_mouse_capture();
            self.mouse_capture = false;
        }
        if self.bracketed_paste {
            let _ = self.io.disable_bracketed_paste();
            self.bracketed_paste = false;
        }
        if self.keyboard_enhancement {
            let _ = self.io.pop_keyboard_enhancement();
            self.keyboard_enhancement = false;
        }
    }
}

impl<I: TerminalIo> Drop for TerminalModes<I> {
    fn drop(&mut self) {
        // Reverse order: the UI's escapes are torn down while raw mode is still
        // on, then ratatui (raw mode off) is restored.
        self.undo_modes();
        // On a panic, ratatui's global panic hook already restored before
        // unwinding reached this `Drop`. Skip our own restore so ratatui is
        // restored exactly once instead of twice. (On a normal or error return
        // path the thread is not panicking, so we are the sole restorer.)
        if !thread::panicking() {
            self.io.restore_ratatui();
        }
    }
}
