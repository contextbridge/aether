use super::{SettingsChange, SettingsMenuValue};
use crate::edit_buffer::EditBuffer;
use crate::filterable_list::FilterableList;
use crate::theme::Theme;
use acp_utils::config_option_id::ConfigOptionId;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget};
use std::collections::HashSet;
use utils::ReasoningEffort;

pub(super) struct ModelSelector {
    config_id: String,
    items: FilterableList<SettingsMenuValue>,
    pub(super) all_items: Vec<SettingsMenuValue>,
    pub(super) selected_models: HashSet<String>,
    original_models: HashSet<String>,
    query: EditBuffer,
    pub(super) filtered: Vec<usize>,
    table_state: TableState,
    reasoning_effort: Option<ReasoningEffort>,
    original_reasoning_effort: Option<ReasoningEffort>,
}

impl ModelSelector {
    pub(super) fn new(
        config_id: String,
        items: Vec<SettingsMenuValue>,
        current_selection: &str,
        current_reasoning_effort: Option<&str>,
    ) -> Self {
        let selected_models: HashSet<String> =
            current_selection.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();

        let reasoning = current_reasoning_effort.and_then(|s| s.parse().ok());
        let original_models = selected_models.clone();
        let original_reasoning_effort = reasoning;

        let filtered: Vec<usize> = (0..items.len()).collect();
        let selected = items
            .iter()
            .position(|value| !value.is_disabled && selected_models.contains(&value.value))
            .or_else(|| items.iter().position(|value| !value.is_disabled));
        let table_state = TableState::default().with_selected(selected);

        Self {
            config_id,
            items: FilterableList::new(items.clone(), |value| format!("{} {}", value.name, value.value)),
            all_items: items,
            selected_models,
            original_models,
            query: EditBuffer::default(),
            filtered,
            table_state,
            reasoning_effort: reasoning,
            original_reasoning_effort,
        }
    }

    pub(super) fn move_up(&mut self) {
        self.move_selection(-1);
    }

    pub(super) fn move_down(&mut self) {
        self.move_selection(1);
    }

    pub(super) fn move_selection(&mut self, direction: isize) {
        let len = self.filtered.len();
        if len == 0 {
            self.table_state.select(None);
            return;
        }
        let selected = self.table_state.selected().unwrap_or_default();
        let next = if direction < 0 { selected.checked_sub(1).unwrap_or(len - 1) } else { (selected + 1) % len };
        self.table_state.select(Some(next));
        self.ensure_enabled();
    }
    pub(super) fn ensure_enabled(&mut self) {
        let selected = self.table_state.selected().unwrap_or_default();
        if self.filtered.get(selected).is_some_and(|&index| self.all_items[index].is_disabled) {
            let enabled = self.filtered.iter().position(|&index| !self.all_items[index].is_disabled);
            self.table_state.select(enabled);
        }
    }

    pub(super) fn toggle_focused(&mut self) {
        if let Some(&value_idx) = self.table_state.selected().and_then(|selected| self.filtered.get(selected)) {
            let v = &self.all_items[value_idx];
            if !v.is_disabled && !self.selected_models.remove(&v.value) {
                self.selected_models.insert(v.value.clone());
            }
        }
    }

    pub(super) fn cycle_reasoning(&mut self) {
        if let Some(&value_idx) = self.table_state.selected().and_then(|selected| self.filtered.get(selected)) {
            let v = &self.all_items[value_idx];
            if !v.is_disabled && !v.meta.reasoning_levels.is_empty() {
                self.reasoning_effort = ReasoningEffort::cycle_within(self.reasoning_effort, &v.meta.reasoning_levels);
            }
        }
    }

    pub(super) fn clamp_reasoning_to_focused(&mut self) {
        if let Some(effort) = self.reasoning_effort
            && let Some(&value_idx) = self.table_state.selected().and_then(|selected| self.filtered.get(selected))
        {
            let v = &self.all_items[value_idx];
            if v.meta.reasoning_levels.is_empty() {
                self.reasoning_effort = None;
            } else {
                self.reasoning_effort = Some(effort.clamp_to(&v.meta.reasoning_levels));
            }
        }
    }

    pub(super) fn push_query_char(&mut self, c: char) {
        self.query.insert_char(c);
        self.refilter();
    }

    pub(super) fn pop_query_char(&mut self) {
        self.query.backspace();
        self.refilter();
    }

    pub(super) fn refilter(&mut self) {
        self.items.set_query(self.query.text());
        self.filtered = self.items.filtered_entries().map(|(index, _)| index).collect();
        self.table_state.select(self.table_state.selected().filter(|selected| *selected < self.filtered.len()));
        self.ensure_enabled();
    }

    pub(super) fn click_row(&mut self, row: usize, _viewport_height: usize) -> bool {
        let Some(item_row) = row.checked_sub(3) else {
            return false;
        };
        let selected = self.table_state.offset().checked_add(item_row);
        if selected.is_none_or(|selected| selected >= self.filtered.len()) {
            return false;
        }
        self.table_state.select(selected);
        true
    }

    pub(super) fn confirm(&self) -> Vec<SettingsChange> {
        let mut changes = Vec::new();
        if !self.selected_models.is_empty() && self.selected_models != self.original_models {
            let joined = self.selected_models.iter().cloned().collect::<Vec<_>>().join(",");
            changes.push(SettingsChange { config_id: self.config_id.clone(), new_value: joined });
        }
        if self.reasoning_effort != self.original_reasoning_effort {
            changes.push(SettingsChange {
                config_id: ConfigOptionId::ReasoningEffort.as_str().to_string(),
                new_value: ReasoningEffort::config_str(self.reasoning_effort).to_string(),
            });
        }
        changes
    }

    pub(super) fn reasoning_label(&self) -> &'static str {
        ReasoningEffort::config_str(self.reasoning_effort)
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let [header_area, table_area] = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).areas(area);
        let selected = self
            .all_items
            .iter()
            .filter(|item| self.selected_models.contains(&item.value))
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        Paragraph::new(format!(" Model search: {}\n Selected: {selected}", self.query.text()))
            .style(Style::new().fg(theme.text_secondary))
            .render(header_area, buffer);

        let rows = self.filtered.iter().map(|&index| {
            let value = &self.all_items[index];
            let selected = self.selected_models.contains(&value.value);
            let availability = if value.is_disabled {
                value
                    .description
                    .as_deref()
                    .and_then(|description| description.strip_prefix("Unavailable: "))
                    .unwrap_or("unavailable")
            } else if selected {
                "selected"
            } else {
                ""
            };
            let capabilities = capability_tags(value.meta.supports_image, value.meta.supports_audio);
            Row::new(vec![
                Cell::from(if selected { "[x]" } else { "[ ]" }),
                Cell::from(model_label(&value.name)),
                Cell::from(capabilities),
                Cell::from(availability),
            ])
            .style(if value.is_disabled {
                Style::new().fg(theme.text_secondary)
            } else {
                Style::new().fg(theme.text_primary)
            })
        });
        self.table_state.select(self.table_state.selected());
        let table = Table::new(
            rows,
            [Constraint::Length(3), Constraint::Min(12), Constraint::Length(10), Constraint::Length(18)],
        )
        .header(Row::new(["", "Model", "Capabilities", "Status"]).style(Style::new().fg(theme.heading)))
        .row_highlight_style(Style::new().fg(theme.background).bg(theme.text_primary));
        StatefulWidget::render(table, table_area, buffer, &mut self.table_state);
    }
}

#[cfg(test)]
pub(super) fn provider_key(value: &str) -> &str {
    if let Some(provider) = value.strip_prefix("__unavailable:") {
        return provider;
    }
    value.split_once(':').map_or("Other", |(p, _)| p)
}

#[cfg(test)]
pub(super) fn provider_label(name: &str, key: &str) -> String {
    if let Some((provider, _)) = name.split_once(" / ") {
        return provider.to_string();
    }
    if key.is_empty() {
        return "Other".to_string();
    }
    let mut chars = key.chars();
    let first = chars.next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    let rest = chars.as_str().to_lowercase();
    format!("{first}{rest}")
}

pub(super) fn model_label(name: &str) -> &str {
    name.split_once(" / ").map_or(name, |(_, model)| model)
}

pub(super) fn capability_tags(supports_image: bool, supports_audio: bool) -> &'static str {
    match (supports_image, supports_audio) {
        (true, true) => "img  audio",
        (true, false) => "img",
        (false, true) => "audio",
        (false, false) => "",
    }
}

#[cfg(test)]
pub(super) fn reasoning_bar(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> String {
    let current_idx = effort.and_then(|e| levels.iter().position(|&l| l == e)).unwrap_or(usize::MAX);
    let parts: Vec<String> = levels
        .iter()
        .enumerate()
        .map(
            |(i, _level)| {
                if i <= current_idx && current_idx != usize::MAX { "■".to_string() } else { "·".to_string() }
            },
        )
        .collect();
    let name = ReasoningEffort::config_str(effort);
    format!("{} [{}]", name, parts.join(""))
}
