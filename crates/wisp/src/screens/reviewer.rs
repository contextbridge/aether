use crate::settings::overlay::SettingsChange;
use crate::surfaces::input::UiOutput;
use crate::view::input::{MouseAction, UiEvent};
use acp_utils::config_option_id::THEME_CONFIG_ID;
use clankerdiff_ratatui::theme::ThemeId;
use crossterm::event::{Event, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

pub(super) fn crossterm_event(event: &UiEvent) -> Event {
    match event {
        UiEvent::Key(key) => Event::Key(*key),
        UiEvent::Paste(text) => Event::Paste(text.clone()),
        &UiEvent::Mouse(action, (column, row)) => {
            let kind = match action {
                MouseAction::ScrollUp => MouseEventKind::ScrollUp,
                MouseAction::ScrollDown => MouseEventKind::ScrollDown,
                MouseAction::Click => MouseEventKind::Down(MouseButton::Left),
            };
            Event::Mouse(MouseEvent { kind, row, column, modifiers: KeyModifiers::NONE })
        }
    }
}

pub(super) fn theme_change(id: ThemeId) -> UiOutput {
    let new_value = match id {
        ThemeId::Custom(name) => format!("file:{name}"),
        id => format!("builtin:{id}"),
    };
    UiOutput::ConfigChange(SettingsChange { config_id: THEME_CONFIG_ID.to_string(), new_value })
}
