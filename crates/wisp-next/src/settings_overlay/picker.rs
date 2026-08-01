use super::{KeyHint, SettingsChange, SettingsMenuEntry, SettingsMenuValue, SettingsPane, message_for_change};
use crate::filterable_list::FilterableList;
use crate::renderer::DrawContext;
use crate::surface::{Action, Surface, SurfaceList};
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

/// Single-select pane: a search box over one config option's values.
pub(super) struct SettingsPicker {
    config_id: String,
    title: String,
    current_value: String,
    values: FilterableList<SettingsMenuValue>,
}

impl SettingsPicker {
    pub(super) fn from_entry(entry: &SettingsMenuEntry) -> Option<Self> {
        let current_value = entry.values.get(entry.current_value_index)?.value.clone();
        let mut values = FilterableList::new(entry.values.clone(), |value| format!("{} {}", value.name, value.value))
            .selectable(|value| !value.is_disabled);
        values.select_index(entry.current_value_index);

        Some(Self { config_id: entry.config_id.clone(), title: entry.title.clone(), current_value, values })
    }

    fn render_pane(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let [header_area, list_area] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
        Paragraph::new(truncate_to_width(
            &format!(" {} search: {}", self.title, self.values.query()),
            usize::from(header_area.width),
        ))
        .style(Style::new().fg(theme.text_secondary))
        .render(header_area, buffer);

        self.values
            .view(theme, |value| {
                let label = if value.name == value.value {
                    value.name.clone()
                } else {
                    format!("{} ({})", value.name, value.value)
                };
                let style = if value.is_disabled {
                    Style::new().fg(theme.text_secondary)
                } else {
                    Style::new().fg(theme.text_primary)
                };
                Line::styled(label, style)
            })
            .pane(" (no matches found)")
            .render(list_area, buffer);
    }
}

impl Surface for SettingsPicker {
    /// Confirms the focused value, returning to the menu. Yields no change when
    /// the value is disabled or already current.
    fn activate(&mut self) -> Vec<Action> {
        let change = self
            .values
            .selected_entry()
            .filter(|value| !value.is_disabled && value.value != self.current_value)
            .map(|value| SettingsChange { config_id: self.config_id.clone(), new_value: value.value.clone() });
        change.iter().map(message_for_change).chain([Action::Close]).collect()
    }

    fn list(&mut self) -> Option<&mut dyn SurfaceList> {
        Some(&mut self.values)
    }

    fn activates_on_click(&self) -> bool {
        true
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, cx: &mut DrawContext<'_>) -> Option<Position> {
        self.render_pane(area, buf, cx.theme);
        None
    }
}

impl SettingsPane for SettingsPicker {
    fn footer(&self) -> Vec<KeyHint> {
        vec![("Enter", "confirm".into()), ("Esc", "back".into())]
    }
}
