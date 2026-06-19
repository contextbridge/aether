A contiguous run of text sharing a single [`Style`].

Spans are the building blocks of [`Line`](crate::Line)s. Each span carries a text string and a style (colors + attributes). When appending to a `Line`, spans with matching styles are automatically merged.

# Construction

```rust,no_run
use tui::{Span, Style, Color};

// Unstyled text
let span = Span::new("hello");

// Styled text
let span = Span::with_style("error", Style::fg(Color::Red).bold());
```

# Methods

- **`text()`** — Borrow the text content.
- **`style()`** — Copy of the span's [`Style`].

In most cases you won't construct `Span`s directly — use [`Line::push_styled`](crate::Line::push_styled) or [`Line::push_with_style`](crate::Line::push_with_style) instead.
