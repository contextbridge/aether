The size this render call may draw into, plus the [`Theme`] in effect.

A `ViewContext` describes an **allocated size**, not the terminal size. When a parent component renders a child, it is responsible for narrowing the context to the child's slot before calling [`Component::render`](crate::Component::render). Children should treat `size` as authoritative — only the root component should assume the full terminal width.

# Size contract

- Parents allocate sizes and pass size-scoped contexts to children.
- Children render into the size they were given. They do not negotiate width upward.
- Composition (stacking, side-by-side layout, padding) happens on [`Frame`](crate::Frame), not by reaching back into the parent's `ViewContext`.

# Construction

```rust,no_run
use tui::ViewContext;

// Default theme
let ctx = ViewContext::new((80, 24));

// Custom theme
use tui::Theme;
let ctx = ViewContext::new_with_theme((120, 40), Theme::default());
```

# Size helpers

When rendering a child into part of the parent's size, derive a new context with one of:

- **`with_size((w, h))`** — Replace the entire size.
- **`with_width(w)`** — Replace just the width, keep the height.
- **`with_height(h)`** — Replace just the height, keep the width.
- **`inset(insets)`** — Shrink the size by [`Insets`](crate::Insets) on each edge. Saturates at zero on each axis.

All helpers preserve the theme and (when the `syntax` feature is enabled) the syntax highlighter.

```rust,no_run
use tui::{Insets, ViewContext};

let parent = ViewContext::new((80, 24));

// Render a child in a left half of width 40.
let left = parent.with_width(40);

// Render a child inside a 2-column horizontal padding.
let padded = parent.inset(Insets::horizontal(2));
```

# `Size`

[`Size`](crate::Size) holds the dimensions of an allocated render area as `width` (columns) and `height` (rows), both `u16`. Implements `From<(u16, u16)>` for convenient construction.

# Other methods

- **`highlighter()`** — Access the [`SyntaxHighlighter`](crate::SyntaxHighlighter) (requires feature `syntax`).

# See also

- [`Frame`](crate::Frame) — The render artifact returned by `Component::render`. Composition primitives like `fit`, `indent`, `vstack`, and `hstack` operate on frames, not on the parent context.
- [`Insets`](crate::Insets) — Edge insets used by `inset()`.
- [`Theme`] — The semantic color palette.
- [`Renderer`](crate::Renderer) — Creates `ViewContext` automatically from its own state.
