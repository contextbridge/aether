Low-level RAII guard for terminal raw mode, bracketed paste, and mouse capture.

Creating a `TerminalSession` enables raw mode (and optionally bracketed paste and mouse capture). When the session is dropped, all terminal state is restored automatically.

Most callers should use [`TerminalRuntime`](crate::TerminalRuntime) instead of managing a `TerminalSession` directly.

# Usage

```rust,no_run
use tui::{TerminalSession, MouseCapture};

let _session = TerminalSession::new(
    true,               // enable bracketed paste
    MouseCapture::Enabled,
)?;
// Terminal is now in raw mode with paste and mouse capture.
// Dropping `_session` restores the terminal.
# Ok::<(), std::io::Error>(())
```

# `MouseCapture`

- **`Disabled`** — No mouse events are reported.
- **`Enabled`** — Mouse clicks, scrolls, and drags are reported as events.

# Related

- **[`TerminalRuntime`](crate::TerminalRuntime)** — High-level runtime that owns the session, renderer, and event source. Preferred for interactive apps.
- **[`terminal_size()`](crate::terminal_size)** — Returns the current terminal dimensions as `(columns, rows)`.
