use super::{SettingsChange, SettingsMenuEntry, SettingsMenuValue, message_for_change, value_match_key};
use crate::surfaces::input::{Nav, SettingsOutput, UiEvent};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::wrap::truncate_to_width;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};

/// Single-select pane: a search box over one config option's values.
pub(super) struct SettingsPicker {
    config_id: String,
    title: String,
    current_value: String,
    values: FilterableList<SettingsMenuValue>,
}

pub(super) struct SettingsPickerView<'a> {
    title: &'a str,
    theme: &'a Theme,
}

impl<'a> SettingsPickerView<'a> {
    pub(super) fn new(title: &'a str, theme: &'a Theme) -> Self {
        Self { title, theme }
    }
}

impl SettingsPicker {
    pub(super) fn from_entry(entry: &SettingsMenuEntry) -> Option<Self> {
        let current_value = entry.values.get(entry.current_value_index)?.value.clone();
        let mut values =
            FilterableList::new(entry.values.clone(), value_match_key).selectable(|value| !value.is_disabled);
        values.select_index(entry.current_value_index);

        Some(Self { config_id: entry.config_id.clone(), title: entry.title.clone(), current_value, values })
    }
}

impl SettingsPicker {
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<SettingsOutput> {
        match self.values.on_nav_event(&event) {
            Nav::Close => vec![SettingsOutput::Close],
            Nav::Activate | Nav::Clicked => {
                let change = self
                    .values
                    .selected_entry()
                    .filter(|value| !value.is_disabled && value.value != self.current_value)
                    .map(|value| SettingsChange { config_id: self.config_id.clone(), new_value: value.value.clone() });
                change.iter().map(message_for_change).chain([SettingsOutput::Close]).collect()
            }
            Nav::Moved | Nav::Unhandled => Vec::new(),
        }
    }
}

impl SettingsPicker {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        let Self { title, values, .. } = self;
        StatefulWidget::render(SettingsPickerView::new(title, theme), area, buf, values);
        None
    }
}

impl StatefulWidget for SettingsPickerView<'_> {
    type State = FilterableList<SettingsMenuValue>;

    fn render(self, area: Rect, buffer: &mut Buffer, state: &mut Self::State) {
        let [header_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(truncate_to_width(
            &format!(" {} search: {}", self.title, state.query()),
            usize::from(header_area.width),
        ))
        .style(Style::new().fg(self.theme.text_secondary))
        .render(header_area, buffer);

        let (view, selection) = state.view(self.theme, |value| {
            let label = if value.name == value.value {
                value.name.clone()
            } else {
                format!("{} ({})", value.name, value.value)
            };
            let style = if value.is_disabled {
                Style::new().fg(self.theme.text_secondary)
            } else {
                Style::new().fg(self.theme.text_primary)
            };
            Line::styled(label, style)
        });
        StatefulWidget::render(
            view.pane(" (no matches found)").highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING),
            list_area,
            buffer,
            selection,
        );
    }
}
