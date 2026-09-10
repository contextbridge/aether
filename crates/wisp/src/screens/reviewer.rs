use crate::surfaces::input::{MouseAction, UiEvent};
use clankerdiff_theme::ThemeId;
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub(super) fn crossterm_event(event: UiEvent) -> Event {
    match event {
        UiEvent::Key(key) => Event::Key(key),
        UiEvent::Paste(text) => Event::Paste(text),
        UiEvent::Mouse(action, (column, row)) => {
            let kind = match action {
                MouseAction::ScrollUp => MouseEventKind::ScrollUp,
                MouseAction::ScrollDown => MouseEventKind::ScrollDown,
                MouseAction::Click => MouseEventKind::Down(MouseButton::Left),
            };
            Event::Mouse(MouseEvent { kind, row, column, modifiers: KeyModifiers::NONE })
        }
    }
}

pub(super) fn theme_selection(id: ThemeId) -> String {
    match id {
        ThemeId::Custom(name) => format!("file:{name}"),
        id => format!("builtin:{id}"),
    }
}
