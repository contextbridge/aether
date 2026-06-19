Tracks which child in a list of focusable items is currently focused.

A `FocusRing` is a simple index tracker with wrap-around cycling. Parent components own it and use it to route events to the focused child and style focused vs. unfocused items during rendering.

# Usage

```rust
use tui::{FocusRing, FocusOutcome};

let mut ring = FocusRing::new(3);
assert_eq!(ring.focused(), 0);

ring.focus_next();
assert_eq!(ring.focused(), 1);

// Wraps around at the end
ring.focus_next();
ring.focus_next();
assert_eq!(ring.focused(), 0);
```

# Key methods

- **`new(len)`** — Create a ring with wrapping enabled, focused at index 0.
- **`without_wrap()`** — Disable wrap-around (builder pattern).
- **`focused()`** / **`is_focused(index)`** — Query the current focus.
- **`focus_next()`** / **`focus_prev()`** — Advance or retreat. Returns `true` if focus changed.
- **`focus(index)`** — Programmatically set focus. Returns `false` if out of bounds.
- **`set_len(len)`** — Update the item count, clamping focus if needed.
- **`handle_key(key_event)`** — Handle Tab/BackTab and return a [`FocusOutcome`].

# `FocusOutcome`

Returned by [`handle_key`](FocusRing::handle_key):

- **`FocusChanged`** — Focus moved to a different index.
- **`Unchanged`** — A focus key was pressed but focus didn't move (e.g. at boundary without wrap).
- **`Ignored`** — The key was not Tab or `BackTab`.
