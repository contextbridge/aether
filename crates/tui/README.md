# tui

A lightweight, composable terminal UI library for building full-screen CLI apps in Rust.

Your app owns its event loop and state machine. The library provides composable building blocks: a [`Component`] trait for widgets, a diff-based [`Renderer`], and RAII terminal management.

## Table of Contents

<!-- START doctoc generated TOC please keep comment here to allow auto update -->
<!-- DON'T EDIT THIS SECTION, INSTEAD RE-RUN doctoc TO UPDATE -->

- [Minimal app](#minimal-app)
- [How it works](#how-it-works)
- [Composing components](#composing-components)
- [Built-in widgets](#built-in-widgets)
- [Feature flags](#feature-flags)
- [License](#license)

<!-- END doctoc generated TOC please keep comment here to allow auto update -->

## Minimal app

A complete TUI app has three parts: a [`TerminalRuntime`] (terminal lifecycle), a [`Component`] (your UI), and a loop that wires them together.

```rust,no_run
use std::io;
use tui::{
    Component, CrosstermEvent, Event, Frame, KeyCode, Line,
    MouseCapture, TerminalConfig, TerminalRuntime, Theme, ViewContext,
    terminal_size,
};

// 1. Define your root component
struct Counter { count: i32 }

impl Component for Counter {
    type Message = CounterMsg;
    async fn on_event(&mut self, event: &Event) -> Option<Vec<CounterMsg>> {
        if let Event::Key(key) = event {
            match key.code {
                KeyCode::Up    => self.count += 1,
                KeyCode::Down  => self.count -= 1,
                KeyCode::Char('q') => return Some(vec![CounterMsg::Quit]),
                _ => return None,
            }
            return Some(vec![]);
        }
        None
    }
    fn render(&mut self, ctx: &ViewContext) -> Frame {
        Frame::new(vec![
            Line::styled("Counter (↑/↓, q to quit)", ctx.theme.muted()),
            Line::new(format!("  {}", self.count)),
        ])
    }
}

enum CounterMsg { Quit }

// 2. Set up the terminal runtime
#[tokio::main]
async fn main() -> io::Result<()> {
    let size = terminal_size().unwrap_or((80, 24));
    let mut terminal = TerminalRuntime::new(
        io::stdout(),
        Theme::default(),
        size,
        TerminalConfig { bracketed_paste: true, mouse_capture: MouseCapture::Disabled },
    )?;

    let mut app = Counter { count: 0 };
    terminal.render_frame(|ctx| app.render(ctx))?; // initial paint

    // 3. Event loop
    loop {
        let Some(raw) = terminal.next_event().await else { break };
        if let CrosstermEvent::Resize(c, r) = &raw {
            terminal.on_resize((*c, *r));
        }
        if let Ok(event) = Event::try_from(raw) {
            if let Some(msgs) = app.on_event(&event).await {
                for msg in msgs {
                    match msg {
                        CounterMsg::Quit => return Ok(()),
                    }
                }
            }
            terminal.render_frame(|ctx| app.render(ctx))?;
        }
    }
    Ok(())
}
```

Dropping the `TerminalRuntime` automatically restores the terminal (disables raw mode, bracketed paste, and mouse capture).

## How it works

```text
crossterm::Event ──→ Event::try_from ──→ Component::on_event ──→ Vec<Message>
                                                │                       │
                                                ▼                       ▼
                                         Component::render     parent handles messages
                                                │
                                                ▼
                                    Renderer::render_frame (diff → ANSI)
```

1. **[`TerminalRuntime::next_event()`]** reads raw crossterm events from an internal blocking task.
2. **[`Event::try_from`]** filters key releases and normalizes resize events.
3. **[`Component::on_event`]** returns `None` (ignored), `Some(vec![])` (consumed), or `Some(vec![msg])` (messages for the parent).
4. **[`Component::render`]** returns a [`Frame`] (lines + cursor) given a [`ViewContext`] (size + theme).
5. **[`TerminalRuntime::render_frame`]** diffs against the previous frame and emits only changed ANSI sequences.

## Composing components

Nest components by owning them in your parent and delegating events:

```rust,no_run
use tui::{Component, Event, Frame, ViewContext, TextField, merge};

struct MyApp {
    name: TextField,
    path: TextField,
    // ...
}

impl Component for MyApp {
    type Message = ();
    async fn on_event(&mut self, event: &Event) -> Option<Vec<()>> {
        // Delegate to the focused child; merge results if needed
        merge(
            self.name.on_event(event).await,
            self.path.on_event(event).await,
        )
    }
    fn render(&mut self, ctx: &ViewContext) -> Frame {
        Frame::vstack([
            self.name.render(ctx),
            self.path.render(ctx),
        ])
    }
}
```

Use [`FocusRing`] to track which child receives events and `Frame::vstack` to stack frames vertically.

## Built-in widgets

| Widget | Description |
|--------|-------------|
| [`TextField`] | Single-line text input |
| [`NumberField`] | Numeric input (integer or float) |
| [`Checkbox`] | Boolean toggle `[x]` / `[ ]` |
| [`RadioSelect`] | Single-select radio buttons |
| [`MultiSelect`] | Multi-select checkboxes |
| [`SelectList`] | Scrollable list with selection |
| [`Form`] | Multi-field tabbed form |
| [`Panel`] | Bordered container |
| [`Spinner`] | Animated progress indicator |
| [`Combobox`] | Fuzzy-searchable picker (feature: `picker`) |

## Feature flags

| Feature | Description | Default |
|---------|-------------|---------|
| `syntax` | Syntax highlighting, markdown rendering, diff previews via syntect | yes |
| `picker` | Fuzzy combobox picker via nucleo | yes |
| `testing` | Test utilities (`TestTerminal`, `render_component`, `assert_buffer_eq`) | no |

Disable defaults for a smaller dependency tree:

```toml
[dependencies]
tui = { version = "0.1", default-features = false }
```

## License

MIT
