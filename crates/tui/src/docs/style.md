Text styling: foreground/background colors and attributes (bold, italic, underline, dim, strikethrough).

`Style` is a value type (`Copy + Default`) used throughout the rendering pipeline. The default style has no colors and no attributes — terminal defaults apply.

# Construction

```rust,no_run
use tui::{Style, Color};

// Foreground only
let s = Style::fg(Color::Red);

// Builder chain
let s = Style::fg(Color::Cyan).bg_color(Color::DarkBlue).bold().italic();

// Start from default
let s = Style::default().bold().underline();
```

# Merging

[`merge`](Style::merge) overlays one style on top of another. `Option` fields (colors) prefer the overlay when `Some`; boolean attributes are OR'd:

```rust,no_run
use tui::{Style, Color};

let base = Style::fg(Color::White).bold();
let overlay = Style::fg(Color::Red); // no bold
let merged = base.merge(overlay);
// merged: fg=Red, bold=true
```

This is used internally when composing [`Span`](crate::Span) styles but is also useful for building theme-derived styles.

# Fields

All fields are public for direct access:

- **`fg`** / **`bg`** — `Option<Color>`. `None` means "inherit terminal default".
- **`bold`** / **`italic`** / **`underline`** / **`dim`** / **`strikethrough`** — boolean attributes.
