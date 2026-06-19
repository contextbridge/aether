A generic scrollable list with keyboard and mouse navigation.

Items must implement the [`SelectItem`] trait, which controls how each row is rendered. The list tracks a selected index with wrapping navigation (Up/Down and mouse scroll).

# Usage

```rust,no_run
use tui::{SelectList, SelectItem, Line, ViewContext};

struct FileEntry { name: String }

impl SelectItem for FileEntry {
    fn render_item(&self, selected: bool, ctx: &ViewContext) -> Line {
        if selected {
            Line::styled(&self.name, ctx.theme.highlight_fg())
        } else {
            Line::new(&self.name)
        }
    }
}

let list = SelectList::new(
    vec![FileEntry { name: "main.rs".into() }],
    "No files",
);
```

# Messages

`SelectList<T>` implements [`Component`](crate::Component) with `Message = SelectListMessage`:

- **`SelectListMessage::Close`** — Emitted on Esc.
- **`SelectListMessage::Select(usize)`** — Emitted on Enter, carrying the selected index.

# Key methods

- **`items()`** / **`items_mut()`** — Access the items.
- **`selected_index()`** / **`selected_item()`** — Query the current selection.
- **`set_items(items)`** — Replace all items, clamping the selection index.
- **`set_selected(index)`** — Programmatically move the selection.
- **`push(item)`** — Append a single item.
- **`retain(f)`** — Filter items in place, clamping the selection.

# `SelectItem` trait

Implement this on your item type to control rendering. The `selected` flag lets you apply highlight styling to the focused row.

# See also

- [`SelectOption`](crate::SelectOption) — A built-in `SelectItem` implementation with `value`, `title`, and optional `description`.
- [`RadioSelect`](crate::RadioSelect) — Single-select radio buttons (used in forms).
