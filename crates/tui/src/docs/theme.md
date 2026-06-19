Semantic color palette for TUI rendering.

A `Theme` provides named colors for every visual role in the UI — text, backgrounds, status indicators, markdown rendering, and diffs. Components access colors through the [`ViewContext`](crate::ViewContext) rather than hardcoding values, making the entire UI re-skinnable.

When the `syntax` feature is enabled, the theme also caches a parsed syntect theme for syntax highlighting.

# Construction

Use the builder for custom themes — every field is required:

```rust,no_run
use tui::{Theme, Color};

let theme = Theme::builder()
    .fg(Color::White)
    .bg(Color::Black)
    .accent(Color::Cyan)
    .highlight_bg(Color::DarkBlue)
    .highlight_fg(Color::White)
    .text_secondary(Color::Grey)
    .code_fg(Color::Green)
    .code_bg(Color::Rgb { r: 30, g: 30, b: 30 })
    .heading(Color::Yellow)
    .link(Color::Blue)
    .blockquote(Color::DarkGrey)
    .muted(Color::DarkGrey)
    .success(Color::Green)
    .warning(Color::Yellow)
    .error(Color::Red)
    .info(Color::Cyan)
    .secondary(Color::Magenta)
    .sidebar_bg(Color::Rgb { r: 20, g: 20, b: 20 })
    .diff_added_fg(Color::Green)
    .diff_removed_fg(Color::Red)
    .diff_added_bg(Color::Rgb { r: 0, g: 20, b: 0 })
    .diff_removed_bg(Color::Rgb { r: 20, g: 0, b: 0 })
    .build()
    .unwrap();
```

`Theme::default()` provides a dark-theme preset.

# Color categories

- **Base** — `fg`, `bg`, `accent`, `highlight_bg`, `highlight_fg`
- **Text** — `text_secondary`, `code_fg`, `code_bg`
- **Markdown** — `heading`, `link`, `blockquote`, `muted`
- **Status** — `success`, `warning`, `error`, `info`, `secondary`
- **Layout** — `sidebar_bg`
- **Diffs** — `diff_added_fg`, `diff_removed_fg`, `diff_added_bg`, `diff_removed_bg`

# Helper methods

- **`selected_row_style()`** — Returns a [`Style`] with `highlight_fg` on `highlight_bg`, suitable for selected list rows.
- **`selected_row_style_with_fg(color)`** — Same but with a custom foreground.

# See also

- `ThemeBuilder` — The builder returned by [`Theme::builder()`].
- [`ThemeBuildError`] — Returned when a required field is missing.
- [`ViewContext`](crate::ViewContext) — Carries an `Arc<Theme>` to render methods.
