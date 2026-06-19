Logical output from a [`Component::render`](crate::Component::render) call: a vector of [`Line`]s plus a [`Cursor`] position.

A `Frame` is the **only render artifact** of a component. Composition between parents and children happens by transforming and stacking frames — not by reaching back into raw `Vec<Line>`. Raw line vectors remain a useful internal convenience inside a single component, but they are not the public composition primitive.

The [`Renderer`](crate::Renderer) consumes frames, diffs them against the previous render, and emits only the changed ANSI sequences.

# Construction

```rust,no_run
use tui::{Frame, Line};
use tui::Cursor;

let frame = Frame::new(vec![
    Line::new("Hello, world!"),
    Line::new("Press q to quit"),
]);

// Optionally place a visible cursor
let frame = frame.with_cursor(Cursor::visible(0, 5));
```

# Composition

All composition operations live on `Frame` itself and own the cursor remapping for the transform they perform.

- **`fit(width, options)`** — Wrap or truncate each row to a target width, optionally extending each row to that width with the row's background color. See [`FitOptions`](crate::FitOptions) and [`Overflow`](crate::Overflow).
- **`indent(cols)`** — Shift visual content `cols` columns to the right and shift the cursor.
- **`vstack(frames)`** — Concatenate frames vertically. The first visible cursor wins; its row is offset by the cumulative line count of preceding frames.
- **`hstack(parts)`** — Compose [`FramePart`](crate::FramePart) slots horizontally into fixed-width columns. Heights are balanced by padding shorter slots with blank rows of the slot's width. The first visible cursor wins; its column is offset by the cumulative width of preceding slots.

These primitives are intentionally small. Containers should compose with them rather than reimplementing wrap, pad, or cursor math by hand.

# Other methods

- **`lines()`** — Borrow the rendered lines.
- **`cursor()`** — The current cursor state.
- **`with_cursor(cursor)`** — Replace the cursor (builder pattern, moves `self`).
- **`clamp_cursor()`** — Clamp the cursor row to the last line index, preventing out-of-bounds positions.
- **`into_lines()`** — Consume the frame and return the lines.
- **`into_parts()`** — Consume and return `(Vec<Line>, Cursor)`.
- **`empty()`** — Construct an empty frame with a hidden cursor.

# See also

- [`Cursor`] — Logical cursor with row, col, and visibility.
- [`Line`] — A single line of styled terminal output.
- [`ViewContext`](crate::ViewContext) — The allocated render region passed into `Component::render`.
- [`Renderer`](crate::Renderer) — Consumes frames and renders them to the terminal.
- [`Frame::vstack`] — Composes multiple frames vertically.
