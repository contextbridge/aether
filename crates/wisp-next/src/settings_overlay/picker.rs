use super::{SettingsChange, SettingsMenuEntry, SettingsMenuValue};
use crate::filterable_list::FilterableList;
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{ListItem, Paragraph, Widget};

pub(super) struct SettingsPicker {
    config_id: String,
    title: String,
    current_value: String,
    pub(super) values: FilterableList<SettingsMenuValue>,
}

impl SettingsPicker {
    pub(super) fn from_entry(entry: &SettingsMenuEntry) -> Option<Self> {
        let current_value = entry.values.get(entry.current_value_index)?.value.clone();
        let mut values = FilterableList::new(entry.values.clone(), |value| format!("{} {}", value.name, value.value));
        values.select_index(entry.current_value_index);

        Some(Self { config_id: entry.config_id.clone(), title: entry.title.clone(), current_value, values })
    }

    pub(super) fn move_up(&mut self) {
        self.values.select_previous();
        self.ensure_selectable();
    }

    pub(super) fn move_down(&mut self) {
        self.values.select_next();
        self.ensure_selectable();
    }

    pub(super) fn push_query_char(&mut self, character: char) {
        self.values.push_query_char(character);
    }

    pub(super) fn pop_query_char(&mut self) {
        self.values.pop_query_char();
    }

    pub(super) fn ensure_selectable(&mut self) {
        if self.values.selected_entry().is_some_and(|value| value.is_disabled) {
            self.values.select_previous();
        }
    }

    pub(super) fn click_row(&mut self, row: usize) -> bool {
        let Some(item_row) = row.checked_sub(1) else {
            return false;
        };
        self.values.select_row(item_row);
        self.values.selected_entry().is_some()
    }

    pub(super) fn confirm_selection(&self) -> Option<SettingsChange> {
        let selected = self.values.selected_entry()?;
        if selected.is_disabled || selected.value == self.current_value {
            return None;
        }
        Some(SettingsChange { config_id: self.config_id.clone(), new_value: selected.value.clone() })
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let [header_area, list_area] = ratatui::layout::Layout::vertical([
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Min(0),
        ])
        .areas(area);
        Paragraph::new(truncate_to_width(
            &format!(" {} search: {}", self.title, self.values.query()),
            usize::from(header_area.width),
        ))
        .style(Style::new().fg(theme.text_secondary))
        .render(header_area, buffer);

        self.values.render_items(
            list_area,
            buffer,
            " (no matches found)",
            Style::new().fg(theme.text_secondary),
            Style::new().fg(theme.background).bg(theme.text_primary),
            |value, _| {
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
                ListItem::new(truncate_to_width(&label, usize::from(list_area.width))).style(style)
            },
        );
    }
}
