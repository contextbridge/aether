Test utilities for components built with this crate.

Requires feature **`testing`**.

# `TestTerminal`

A virtual terminal buffer that captures writes and parses ANSI escape sequences. Use it to assert on rendered output without a real terminal:

```rust,no_run
use tui::testing::{TestTerminal, assert_buffer_eq, render_component};
use tui::{Frame, Line, ViewContext};

let terminal = render_component(
    |ctx: &ViewContext| Frame::new(vec![Line::new("hello")]),
    80, 24,
);
assert_buffer_eq(&terminal, &["hello"]);
```

`TestTerminal` implements `std::io::Write` and tracks cursor position, SGR styles, and scrollback — including delayed (DEC-style) wrapping behavior.

# Helper functions

- **`render_component(f, width, rows)`** — Render a closure through a [`Renderer`](crate::Renderer) into a `TestTerminal`.
- **`render_component_with_renderer(f, renderer, width, rows)`** — Same, but reuse an existing renderer (useful for testing frame diffs).
- **`render_lines(lines, width, rows)`** — Render a slice of [`Line`](crate::Line)s into a `TestTerminal`.
- **`key(code)`** — Create a `KeyEvent` with no modifiers for test input.
- **`sample_options()`** — Returns a `Vec<SelectOption>` with three entries ("Alpha", "Beta", "Gamma").
- **`assert_buffer_eq(terminal, expected)`** — Assert that the terminal buffer matches expected row strings.

# `Cell`

A single cell in the `TestTerminal` buffer, storing a `char` and its [`Style`](crate::Style). Access cells through `TestTerminal::buffer()` for fine-grained style assertions.
