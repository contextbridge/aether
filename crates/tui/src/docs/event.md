Unified input events for [`Component::on_event`](crate::Component::on_event).

Wraps the raw crossterm event types into a smaller, focused set. Convert from [`crossterm::event::Event`](crate::CrosstermEvent) using `TryFrom` — this filters out key releases and maps resize events automatically.

# Variants

- **`Key(KeyEvent)`** — A key press or repeat. Key releases are filtered out during conversion.
- **`Paste(String)`** — Bracketed paste content (requires [`TerminalSession`](crate::TerminalSession) with bracketed paste enabled).
- **`Mouse(MouseEvent)`** — Mouse input (scroll, click, drag). Requires mouse capture.
- **`Tick`** — A periodic timer tick, useful for animations (e.g. [`Spinner`](crate::Spinner)).
- **`Resize(Size)`** — The terminal was resized to a new [`Size`](crate::Size).

# Conversion

```rust,no_run
use tui::Event;
use crossterm::event::Event as CrosstermEvent;

fn poll_event(raw: CrosstermEvent) -> Option<Event> {
    Event::try_from(raw).ok()
}
```

The `TryFrom` conversion returns `Err(())` for events that components should not handle (key releases, focus gained/lost).
