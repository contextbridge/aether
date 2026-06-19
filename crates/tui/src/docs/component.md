The core abstraction for all interactive widgets. Every built-in widget implements this trait, and parent components compose children through it.

The trait follows an event-message design: [`on_event`](Component::on_event) receives input and optionally produces typed messages that bubble up to the parent. [`render`](Component::render) produces a [`Frame`] from the current state.

# Event handling contract

[`on_event`](Component::on_event) returns one of three outcomes:

- **`None`** — the event was not recognized. The parent should propagate it elsewhere.
- **`Some(vec![])`** — the event was consumed but produced no messages (e.g. internal cursor movement).
- **`Some(vec![msg, ...])`** — the event was consumed and produced one or more messages for the parent to handle.

Use [`merge`](crate::merge) to combine outcomes from multiple children.

# Rendering

[`render`](Component::render) receives a [`ViewContext`](crate::ViewContext) — the **allocated render region** plus theme — and returns a [`Frame`](crate::Frame), a vector of [`Line`](crate::Line)s plus an optional [`Cursor`](crate::Cursor) position.

Parents are responsible for narrowing the context to the child's slot before calling `render` (using [`with_width`](crate::ViewContext::with_width), [`with_size`](crate::ViewContext::with_size), or [`inset`](crate::ViewContext::inset)). Children should treat `ctx.size` as authoritative — only the root component should assume the full terminal width. Composition between siblings happens on `Frame` itself via [`fit`](crate::Frame::fit), [`indent`](crate::Frame::indent), [`vstack`](crate::Frame::vstack), and [`hstack`](crate::Frame::hstack), not by reaching back into the parent's context.

# Usage

```rust,no_run
use tui::{Component, Event, Frame, KeyCode, Line, ViewContext};

struct Counter { count: i32 }

impl Component for Counter {
    type Message = ();
    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Up => self.count += 1,
                KeyCode::Down => self.count -= 1,
                _ => return None,
            }
            return Some(vec![]);
        }
        None
    }
    fn render(&mut self, _ctx: &ViewContext) -> Frame {
        Frame::new(vec![Line::new(format!("Count: {}", self.count))])
    }
}
```

# See also

- [`Event`] — the input events passed to `on_event`.
- [`Frame`] — the output of `render`.
- [`ViewContext`] — the environment passed to `render`.
- [`merge`](crate::merge) — combine event outcomes from multiple children.
