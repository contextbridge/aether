A single line of styled terminal output, composed of [`Span`]s.

Each `Span` carries its own [`Style`], so a single `Line` can mix colors, bold, italic, etc. ANSI escape codes are emitted only when [`to_ansi_string`](Line::to_ansi_string) is called, keeping the data model free of formatting concerns.

# Construction

```rust,no_run
use tui::{Line, Style, Color};

// Plain text
let line = Line::new("hello");

// Single color
let line = Line::styled("error", Color::Red);

// Full style
let line = Line::with_style("warning", Style::fg(Color::Yellow).bold());

// Incremental building
let mut line = Line::default();
line.push_text("Name: ");
line.push_styled("Alice", Color::Cyan);
```

# Key methods

- **`push_span(span)`** — Append a [`Span`]. Merges with the last span if styles match.
- **`push_text(text)`** / **`push_styled(text, color)`** / **`push_with_style(text, style)`** — Convenience wrappers over `push_span`.
- **`prepend(text)`** — Insert unstyled text at the front, inheriting background color or fill style from the line.
- **`append_line(other)`** — Append all spans from another `Line`.
- **`display_width()`** — Width in terminal columns (accounts for Unicode).
- **`soft_wrap(width)`** — Break into multiple `Line`s fitting within `width` columns. Fill metadata is propagated to each wrapped row.
- **`to_ansi_string()`** — Emit the line as an ANSI-escaped string.
- **`extend_bg_to_width(target)`** — Pad with spaces to fill `target` columns. If [`with_fill`](Line::with_fill) was called, that color is consumed for the padding; otherwise the existing span background is reused.

# Row fill (deferred padding)

A row can be marked with a **fill background** that tells later layout stages "extend this row to its containing width with trailing spaces in this color." Materialization is deferred until either [`Frame::hstack`](crate::Frame::hstack) (per slot width) or [`VisualFrame::from_frame`](crate::Renderer) (per terminal width) needs it. This avoids the trailing-space wrap artifact: a fill-marked row that gets soft-wrapped at a smaller width does not produce phantom rows from the would-be padding spaces.

- **`with_fill(color)`** — Builder: mark this row as filling its containing width with `color`.
- **`set_fill(Option<Color>)`** — Set or clear the row's fill background; pass `None` to drop existing fill.
- **`fill()`** — Inspect the current fill background as `Option<Color>`.

`Line` implements `Display` for plain-text output (no ANSI codes).
