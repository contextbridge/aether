Diff-based terminal renderer that efficiently updates only changed content.

`Renderer` owns a writable output (anything implementing `std::io::Write`), a [`Theme`], and the previous frame state. On each [`render_frame`](Renderer::render_frame) call it diffs the new frame against the previous one and emits only the changed ANSI sequences.

# Cursor invariant

Uses relative cursor movement (`MoveUp` + `\r`) to navigate back to the start of the managed region. This avoids absolute row tracking, which breaks when the terminal scrolls content upward. After every render or [`push_to_scrollback`](Renderer::push_to_scrollback), the cursor sits at the end of the last managed line unless explicitly repositioned.

# Usage

```rust,no_run
use tui::{Renderer, Theme, Frame, Line};

let mut renderer = Renderer::new(std::io::stdout(), Theme::default(), (80, 24));

renderer.render_frame(|ctx| {
    Frame::new(vec![
        Line::new("Hello, world!"),
        Line::styled("Status: OK", ctx.theme.success()),
    ])
}).unwrap();
```

# Key methods

- **`render_frame(f)`** — Render a frame. The closure receives a [`ViewContext`] and returns a [`Frame`].
- **`push_to_scrollback(lines)`** — Flush lines into terminal scrollback (they become non-managed history).
- **`on_resize(size)`** — Notify the renderer of a terminal resize. The next render will clear and redraw.
- **`clear_screen()`** — Clear the entire viewport and scrollback.
- **`set_theme(theme)`** — Replace the active [`Theme`].
- **`apply_commands(cmds)`** — Execute a batch of [`RendererCommand`]s.
- **`context()`** — Get the current [`ViewContext`] without rendering.

# `RendererCommand`

Commands that can be batched via [`apply_commands`](Renderer::apply_commands):

- **`Bell`** — Emit a terminal BEL (`\x07`).
- **`ClearScreen`** — Clear viewport and scrollback.
- **`SetTheme(Theme)`** — Replace the active theme.
- **`SetMouseCapture(bool)`** — Enable or disable mouse capture.

# See also

- [`Frame`] — The logical output consumed by the renderer.
- [`Theme`] — The color palette used to build [`ViewContext`].
- [`TerminalSession`](crate::TerminalSession) — Manages raw-mode lifecycle.
