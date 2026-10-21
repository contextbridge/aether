use super::{SettingsChange, SettingsMenuEntry, value_match_key};
use crate::surfaces::input::UiOutput;
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::input::{Nav, UiEvent};
use crate::view::wrap::truncate_to_width;
use client_core::session_config_view::LocalConfigValue;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};

/// Single-select pane: a search box over one config option's values.
pub(super) struct SettingsPicker {
    config_id: String,
    title: String,
    current_value: String,
    values: FilterableList<LocalConfigValue>,
}

impl SettingsPicker {
    pub(super) fn from_entry(entry: &SettingsMenuEntry) -> Option<Self> {
        let current_value_index =
            entry.values.iter().position(|value| value.value == entry.current_raw_value).unwrap_or(0);
        let current_value = entry.values.get(current_value_index)?.value.clone();
        let mut values =
            FilterableList::new(entry.values.clone(), value_match_key).selectable(|value| !value.is_disabled);
        values.select_index(current_value_index);

        Some(Self { config_id: entry.config_id.clone(), title: entry.title.clone(), current_value, values })
    }
}

impl SettingsPicker {
    pub(crate) fn on_ui_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
        match self.values.on_nav_event(event) {
            Nav::Activate | Nav::Clicked => {
                let change =
                    self.values.selected_entry().filter(|value| value.value != self.current_value).map(|value| {
                        SettingsChange { config_id: self.config_id.clone(), new_value: value.value.clone() }
                    });
                change.map(UiOutput::ConfigChange).into_iter().chain([UiOutput::Close]).collect()
            }
            Nav::Close | Nav::Moved | Nav::Unhandled => Vec::new(),
        }
    }
}

impl SettingsPicker {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let [header_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(truncate_to_width(
            &format!(" {} search: {}", self.title, self.values.query()),
            usize::from(header_area.width),
        ))
        .style(theme.text_secondary)
        .render(header_area, buf);

        let (view, selection) = self.values.view(theme, |value| {
            let label = if value.name == value.value {
                value.name.clone()
            } else {
                format!("{} ({})", value.name, value.value)
            };
            let style =
                if value.is_disabled { Style::from(theme.text_secondary) } else { Style::from(theme.text_primary) };
            Line::styled(label, style)
        });
        StatefulWidget::render(
            view.pane(" (no matches found)").highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING),
            list_area,
            buf,
            selection,
        );
    }
}
