use super::{PaneOutcome, SettingsChange, SettingsMenuEntry, SettingsMenuValue, SettingsPane};
use crate::filterable_list::FilterableList;
use crate::selection::Direction;
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{ListItem, Paragraph, Widget};

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
        let mut values = FilterableList::new(entry.values.clone(), |value| format!("{} {}", value.name, value.value));
        values.select_index(entry.current_value_index);

        Some(Self { config_id: entry.config_id.clone(), title: entry.title.clone(), current_value, values })
    }

    /// Confirms the focused value, returning to the menu. Yields no change when
    /// the value is disabled or already current.
    fn confirm(&self) -> PaneOutcome {
        let change =
            self.values.selected_entry().filter(|value| !value.is_disabled && value.value != self.current_value);
        PaneOutcome {
            changes: change
                .map(|value| SettingsChange { config_id: self.config_id.clone(), new_value: value.value.clone() })
                .into_iter()
                .collect(),
            back: true,
            ..PaneOutcome::default()
        }
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
            .view(theme, " (no matches found)", |value| {
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
            })
            .highlight_style(Style::new().fg(theme.background).bg(theme.text_primary))
            .render(list_area, buffer);
    }
}

impl SettingsPane for SettingsPicker {
    fn on_key(&mut self, key: KeyEvent) -> PaneOutcome {
        match key.code {
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Enter => return self.confirm(),
            KeyCode::Backspace => self.values.pop_query_char(),
            KeyCode::Char(character) if !character.is_control() => self.values.push_query_char(character),
            _ => {}
        }
        PaneOutcome::default()
    }

    fn click(&mut self, row: usize, _height: usize) -> PaneOutcome {
        // Row 0 is the search header.
        let Some(item_row) = row.checked_sub(1) else {
            return PaneOutcome::default();
        };
        self.values.select_row(item_row);
        if self.values.selected_entry().is_none() {
            return PaneOutcome::default();
        }
        self.confirm()
    }

    fn scroll(&mut self, direction: Direction) {
        self.values.step(direction, |value| !value.is_disabled);
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        self.render_pane(area, buf, theme);
    }

    fn footer(&self) -> String {
        "[Enter] Confirm  [Esc] Back".to_string()
    }
}
