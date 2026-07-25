use super::{SettingsChange, SettingsMenuEntry, SettingsMenuEntryKind, SettingsMenuValue};
use crate::selection::{Direction, SelectionState};
use crate::session_config_view::{as_select, select_values};
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::SessionConfigOption;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{List, ListItem, Paragraph, StatefulWidget, Widget};

pub(super) struct SettingsMenu {
    pub(super) entries: Vec<SettingsMenuEntry>,
    pub(super) selection: SelectionState,
}

impl SettingsMenu {
    pub(super) fn from_config_options(options: &[SessionConfigOption]) -> Self {
        let entries: Vec<SettingsMenuEntry> = options
            .iter()
            .filter(|opt| opt.id.0.as_ref() != ConfigOptionId::ReasoningEffort.as_str())
            .filter_map(|opt| {
                let select = as_select(opt)?;
                let flat_options = select_values(select);
                if flat_options.is_empty() {
                    return None;
                }

                let current_value_index =
                    flat_options.iter().position(|o| o.value == select.current_value).unwrap_or(0);

                let values: Vec<SettingsMenuValue> = flat_options
                    .into_iter()
                    .map(|o| SettingsMenuValue {
                        value: o.value.0.to_string(),
                        name: o.name.clone(),
                        is_disabled: o.description.as_deref().is_some_and(|d| d.starts_with("Unavailable:")),
                        description: o.description.clone(),
                        meta: SelectOptionMeta::from_meta(o.meta.as_ref()),
                    })
                    .collect();

                let multi_select = ConfigOptionMeta::from_meta(opt.meta.as_ref()).multi_select;

                let display_name = if multi_select && select.current_value.0.contains(',') {
                    let parts: Vec<&str> = select.current_value.0.split(',').map(str::trim).collect();
                    let names: Vec<&str> = parts
                        .iter()
                        .filter_map(|val| values.iter().find(|v| v.value == *val).map(|v| v.name.as_str()))
                        .collect();
                    if names.is_empty() { Some(format!("{} models", parts.len())) } else { Some(names.join(", ")) }
                } else {
                    None
                };

                Some(SettingsMenuEntry {
                    config_id: opt.id.0.to_string(),
                    title: opt.name.clone(),
                    values,
                    current_value_index,
                    current_raw_value: select.current_value.0.to_string(),
                    entry_kind: SettingsMenuEntryKind::Select,
                    multi_select,
                    display_name,
                })
            })
            .collect();

        let selection = SelectionState::new(entries.len());
        Self { entries, selection }
    }

    pub(super) fn move_up(&mut self) {
        self.step(Direction::Backward);
    }

    pub(super) fn move_down(&mut self) {
        self.step(Direction::Forward);
    }

    pub(super) fn step(&mut self, direction: Direction) {
        self.selection.step(self.entries.len(), direction, |_| true);
    }

    pub(super) fn selected_entry(&self) -> Option<&SettingsMenuEntry> {
        self.selection.selected().and_then(|selected| self.entries.get(selected))
    }

    pub(super) fn update_options(&mut self, options: &[SessionConfigOption]) {
        let local_entries: Vec<SettingsMenuEntry> =
            self.entries.iter().filter(|e| !matches!(e.entry_kind, SettingsMenuEntryKind::Select)).cloned().collect();
        *self = Self::from_config_options(options);
        self.entries.splice(0..0, local_entries);
        self.selection.clamp(self.entries.len());
    }

    pub(super) fn apply_change(&mut self, change: &SettingsChange) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.config_id == change.config_id) {
            entry.current_raw_value.clone_from(&change.new_value);
            if let Some(index) = entry.values.iter().position(|v| v.value == change.new_value) {
                entry.current_value_index = index;
            }
        }
    }

    /// Selects the entry drawn at terminal `row`, reporting whether one was hit.
    pub(super) fn click_at(&mut self, row: u16) -> bool {
        self.selection.select_at(row, self.entries.len())
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        if self.entries.is_empty() {
            Paragraph::new(" (no settings options)").style(Style::new().fg(theme.text_secondary)).render(area, buffer);
            return;
        }

        let items = self.entries.iter().map(|entry| {
            let current_name = entry
                .display_name
                .as_deref()
                .or_else(|| entry.values.get(entry.current_value_index).map(|value| value.name.as_str()))
                .unwrap_or("?");
            ListItem::new(format!(
                " {}",
                truncate_to_width(
                    &format!("{}: {current_name}", entry.title),
                    usize::from(area.width).saturating_sub(2),
                )
            ))
            .style(Style::new().fg(theme.text_primary))
        });
        let list = List::new(items)
            .highlight_style(Style::new().fg(theme.background).bg(theme.text_primary))
            .scroll_padding(1);
        self.selection.set_rows_area(area);
        StatefulWidget::render(list, area, buffer, self.selection.list_state_mut());
    }

    /// Adds or refreshes a row that opens a pane instead of picking a value.
    /// Its "current value" is just the summary line shown in the menu.
    pub(super) fn upsert_pane_entry(&mut self, kind: SettingsMenuEntryKind, summary: &str) {
        let (config_id, title) = match kind {
            SettingsMenuEntryKind::McpServers => ("_mcp_servers", "MCP Servers"),
            SettingsMenuEntryKind::ProviderLogins => ("_provider_logins", "Provider Logins"),
            SettingsMenuEntryKind::Select | SettingsMenuEntryKind::Theme => return,
        };
        let entry = SettingsMenuEntry {
            config_id: config_id.to_string(),
            title: title.to_string(),
            values: vec![SettingsMenuValue {
                value: summary.to_string(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: summary.to_string(),
            entry_kind: kind,
            multi_select: false,
            display_name: None,
        };

        match self.entries.iter().position(|existing| existing.entry_kind == kind) {
            Some(index) => self.entries[index] = entry,
            None => self.entries.push(entry),
        }
    }
}
