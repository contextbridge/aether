use super::{SettingsChange, SettingsMenuEntry, SettingsMenuEntryKind, SettingsMenuValue};
use crate::selection::SelectionState;
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::{SessionConfigKind, SessionConfigOption, SessionConfigSelectOptions};
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
                let SessionConfigKind::Select(ref select) = opt.kind else {
                    return None;
                };

                let flat_options = match &select.options {
                    SessionConfigSelectOptions::Ungrouped(opts) => opts.clone(),
                    SessionConfigSelectOptions::Grouped(groups) => {
                        groups.iter().flat_map(|g| g.options.clone()).collect()
                    }
                    _ => return None,
                };

                if flat_options.is_empty() {
                    return None;
                }

                let current_value_index =
                    flat_options.iter().position(|o| o.value == select.current_value).unwrap_or(0);

                let values: Vec<SettingsMenuValue> = flat_options
                    .into_iter()
                    .map(|o| SettingsMenuValue {
                        value: o.value.0.to_string(),
                        name: o.name,
                        is_disabled: o.description.as_deref().is_some_and(|d| d.starts_with("Unavailable:")),
                        description: o.description,
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
        self.selection.previous(self.entries.len());
    }

    pub(super) fn move_down(&mut self) {
        self.selection.next(self.entries.len());
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

    pub(super) fn click_row(&mut self, row: usize) -> Option<SettingsMenuEntryKind> {
        self.selection.select_row(row, self.entries.len());
        self.selected_entry().map(|entry| entry.entry_kind)
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
        StatefulWidget::render(list, area, buffer, self.selection.list_state_mut());
    }

    pub(super) fn upsert_mcp_servers_entry(&mut self, summary: &str) {
        let entry = SettingsMenuEntry {
            config_id: "_mcp_servers".to_string(),
            title: "MCP Servers".to_string(),
            values: vec![SettingsMenuValue {
                value: summary.to_string(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: summary.to_string(),
            entry_kind: SettingsMenuEntryKind::McpServers,
            multi_select: false,
            display_name: None,
        };

        if let Some(pos) = self.entries.iter().position(|e| matches!(e.entry_kind, SettingsMenuEntryKind::McpServers)) {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub(super) fn upsert_provider_logins_entry(&mut self, summary: &str) {
        let entry = SettingsMenuEntry {
            config_id: "_provider_logins".to_string(),
            title: "Provider Logins".to_string(),
            values: vec![SettingsMenuValue {
                value: summary.to_string(),
                name: summary.to_string(),
                description: None,
                is_disabled: false,
                meta: SelectOptionMeta::default(),
            }],
            current_value_index: 0,
            current_raw_value: summary.to_string(),
            entry_kind: SettingsMenuEntryKind::ProviderLogins,
            multi_select: false,
            display_name: None,
        };

        if let Some(pos) =
            self.entries.iter().position(|e| matches!(e.entry_kind, SettingsMenuEntryKind::ProviderLogins))
        {
            self.entries[pos] = entry;
        } else {
            self.entries.push(entry);
        }
    }
}
