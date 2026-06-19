A fuzzy-searchable picker that filters items as the user types.

`Combobox` wraps a [`FuzzyMatcher`](crate::FuzzyMatcher) and adds selection tracking with scroll offset for large result sets. Items must implement the [`Searchable`](crate::Searchable) trait.

Requires feature **`picker`**.

# Usage

```rust,no_run
use tui::{Combobox, Searchable};

#[derive(Clone)]
struct Tool { name: String }

impl Searchable for Tool {
    fn search_text(&self) -> String { self.name.clone() }
}

let picker = Combobox::new(vec![
    Tool { name: "read_file".into() },
    Tool { name: "write_file".into() },
    Tool { name: "list_dir".into() },
]);
```

# Event handling

`Combobox` implements [`Component`](crate::Component) with `Message = PickerMessage<T>`. Use [`classify_key`](crate::classify_key) to map raw key events to [`PickerKey`](crate::PickerKey) variants for custom handling outside the component.

Key messages:

- **`PickerMessage::Confirm(T)`** — User selected an item (Enter).
- **`PickerMessage::Close`** — User dismissed the picker (Esc).
- **`PickerMessage::CharTyped(char)`** — A character was typed (updates the fuzzy query).
- **`PickerMessage::PopChar`** — Backspace pressed.
- **`PickerMessage::CloseAndPopChar`** / **`CloseWithChar(char)`** — Close and forward the key to the parent.

# Key methods

- **`query()`** — The current search string.
- **`matches()`** — The filtered and ranked items.
- **`selected_index()`** — Index into `matches()`.
- **`selected_item()`** — The currently highlighted item.

# See also

- [`Searchable`](crate::Searchable) — Trait items must implement for fuzzy matching.
- [`FuzzyMatcher`](crate::FuzzyMatcher) — The underlying matching engine.
- [`PickerKey`](crate::PickerKey) — Classified key event variants.
