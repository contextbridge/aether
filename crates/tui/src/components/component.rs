use crossterm::event::{KeyEvent, KeyEventKind, MouseEvent};

use crate::rendering::frame::Frame;
use crate::rendering::render_context::{Size, ViewContext};

#[doc = include_str!("../docs/event.md")]
pub enum Event {
    Key(KeyEvent),
    Paste(String),
    Mouse(MouseEvent),
    Tick,
    Resize(Size),
}

impl TryFrom<crossterm::event::Event> for Event {
    type Error = ();

    fn try_from(event: crossterm::event::Event) -> Result<Self, ()> {
        match event {
            crossterm::event::Event::Key(key) if matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) => {
                Ok(Event::Key(key))
            }
            crossterm::event::Event::Paste(text) => Ok(Event::Paste(text)),
            crossterm::event::Event::Mouse(mouse) => Ok(Event::Mouse(mouse)),
            crossterm::event::Event::Resize(cols, rows) => Ok(Event::Resize((cols, rows).into())),
            _ => Err(()),
        }
    }
}

#[doc = include_str!("../docs/component.md")]
pub trait Component {
    /// The message type emitted by this widget.
    type Message;

    /// Process an event and return the outcome.
    ///
    /// - `None` — event not recognized, propagate to parent
    /// - `Some(vec![])` — event consumed, no messages
    /// - `Some(vec![msg, ...])` — event consumed, emit messages
    fn on_event(&mut self, _event: &Event) -> impl Future<Output = Option<Vec<Self::Message>>> {
        async { None }
    }

    /// Render the current state to a frame.
    fn render(&mut self, ctx: &ViewContext) -> Frame;
}

/// Merge two event outcomes. `None` (ignored) yields to the other.
/// Messages are concatenated in order.
pub fn merge<M>(a: Option<Vec<M>>, b: Option<Vec<M>>) -> Option<Vec<M>> {
    match (a, b) {
        (None, other) | (other, None) => other,
        (Some(mut a), Some(b)) => {
            a.extend(b);
            Some(a)
        }
    }
}

/// Generic message type for picker components.
pub enum PickerMessage<T> {
    Close,
    CloseAndPopChar,
    CloseWithChar(char),
    Confirm(T),
    CharTyped(char),
    PopChar,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEventState, KeyModifiers};

    #[test]
    fn try_from_key_press_succeeds() {
        let crossterm_event = crossterm::event::Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
        let event = Event::try_from(crossterm_event);
        assert!(matches!(event, Ok(Event::Key(_))));
    }

    #[test]
    fn try_from_key_release_fails() {
        let crossterm_event = crossterm::event::Event::Key(KeyEvent {
            code: KeyCode::Char('a'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert!(Event::try_from(crossterm_event).is_err());
    }

    #[test]
    fn try_from_paste_succeeds() {
        let crossterm_event = crossterm::event::Event::Paste("hello".to_string());
        let event = Event::try_from(crossterm_event);
        assert!(matches!(event, Ok(Event::Paste(text)) if text == "hello"));
    }

    #[test]
    fn try_from_resize_succeeds() {
        let crossterm_event = crossterm::event::Event::Resize(80, 24);
        let event = Event::try_from(crossterm_event);
        assert!(matches!(event, Ok(Event::Resize(size)) if size.width == 80 && size.height == 24));
    }

    #[test]
    fn try_from_focus_gained_fails() {
        let crossterm_event = crossterm::event::Event::FocusGained;
        assert!(Event::try_from(crossterm_event).is_err());
    }
}
