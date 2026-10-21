use super::{LiveSettingsData, PaneKind, SettingsChange, SettingsMenuEntry};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::list_view::ListView;
use crate::view::selection::SelectionState;
use acp_utils::config_option_id::ConfigOptionId;
use client_core::session_config_view::{LocalConfigKind, LocalConfigOption, LocalConfigValue};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;

pub(super) struct SettingsMenu {
    pub(super) rows: Vec<MenuRow>,
    pub(super) selection: SelectionState,
}

/// One row of the settings menu.
#[derive(Clone)]
pub(super) enum MenuRow {
    /// A config option, opening a picker over its values.
    Select(SettingsMenuEntry),
    /// A row that opens a pane rather than picking a value, summarizing the
    /// pane's live data beside its title.
    Pane(PaneKind),
}

impl MenuRow {
    /// The `title: value` line drawn for this row.
    fn label(&self, live: &LiveSettingsData) -> String {
        match self {
            Self::Select(entry) => {
                let combined = entry
                    .multi_select
                    .then(|| combined_display_name(&entry.current_raw_value, &entry.values))
                    .flatten();
                let current = combined
                    .as_deref()
                    .or_else(|| {
                        entry
                            .values
                            .iter()
                            .find(|value| value.value == entry.current_raw_value)
                            .or_else(|| entry.values.first())
                            .map(|value| value.name.as_str())
                    })
                    .unwrap_or("?");
                format!("{}: {current}", entry.title)
            }
            Self::Pane(kind) => format!("{}: {}", kind.title(), kind.summary(live)),
        }
    }
}

impl SettingsMenu {
    pub(super) fn from_config_options(options: &[LocalConfigOption]) -> Self {
        let rows: Vec<MenuRow> = options
            .iter()
            .filter(|option| option.id != ConfigOptionId::ReasoningEffort.as_str())
            .filter_map(|option| menu_entry(option).map(MenuRow::Select))
            .collect();
        let selection = SelectionState::new(rows.len());
        Self { rows, selection }
    }

    pub(super) fn selected_row(&self) -> Option<&MenuRow> {
        self.selection.selected().and_then(|selected| self.rows.get(selected))
    }

    /// Adds or refreshes rows that are not part of the agent's schema, above the
    /// ones that are. Refreshing in place matters for a row whose values arrive
    /// after it is drawn — the theme list is read off disk — because a second row
    /// for the same option would leave the user a copy that never fills in.
    pub(super) fn upsert_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        let mut added = Vec::new();
        for entry in entries {
            let existing = self.rows.iter().position(
                |row| matches!(row, MenuRow::Select(select) if select.local && select.config_id == entry.config_id),
            );
            match existing {
                Some(index) => self.rows[index] = MenuRow::Select(entry),
                None => added.push(MenuRow::Select(entry)),
            }
        }
        self.rows.splice(0..0, added);
        self.selection.clamp(self.rows.len());
    }

    /// Replaces the agent-provided rows, keeping local and pane rows.
    pub(super) fn update_options(&mut self, options: &[LocalConfigOption]) {
        let preserved: Vec<MenuRow> =
            self.rows.iter().filter(|row| !matches!(row, MenuRow::Select(entry) if !entry.local)).cloned().collect();
        *self = Self::from_config_options(options);
        self.rows.splice(0..0, preserved);
        self.selection.clamp(self.rows.len());
    }

    pub(super) fn apply_change(&mut self, change: &SettingsChange) {
        let entry = self.rows.iter_mut().find_map(|row| match row {
            MenuRow::Select(entry) if entry.config_id == change.config_id => Some(entry),
            _ => None,
        });
        if let Some(entry) = entry {
            entry.current_raw_value.clone_from(&change.new_value);
        }
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme, live: &LiveSettingsData) {
        let rows =
            self.rows.iter().map(|row| Line::styled(format!(" {}", row.label(live)), theme.text_primary)).collect();
        let view = ListView::new(rows, theme)
            .pane(" (no settings options)")
            .highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING);
        StatefulWidget::render(view, area, buffer, &mut self.selection);
    }

    /// Adds or removes the row that opens `kind`'s pane.
    pub(super) fn show_pane_row(&mut self, kind: PaneKind, shown: bool) {
        let existing = self.rows.iter().position(|row| matches!(row, MenuRow::Pane(existing) if *existing == kind));
        match (existing, shown) {
            (None, true) => self.rows.push(MenuRow::Pane(kind)),
            (Some(index), false) => {
                self.rows.remove(index);
            }
            _ => {}
        }
        self.selection.clamp(self.rows.len());
    }
}

fn menu_entry(option: &LocalConfigOption) -> Option<SettingsMenuEntry> {
    let LocalConfigKind::Select { current_value, values, multi_select } = &option.kind else {
        return None;
    };
    if values.is_empty() {
        return None;
    }

    Some(SettingsMenuEntry {
        config_id: option.id.clone(),
        title: option.name.clone(),
        current_raw_value: current_value.clone(),
        values: values.clone(),
        multi_select: *multi_select,
        local: false,
    })
}

/// The label for a multi-select whose value names several options at once,
/// falling back to a count when none of them are known.
fn combined_display_name(current: &str, values: &[LocalConfigValue]) -> Option<String> {
    if !current.contains(',') {
        return None;
    }
    let parts: Vec<&str> = current.split(',').map(str::trim).collect();
    let names: Vec<&str> = parts
        .iter()
        .filter_map(|part| values.iter().find(|value| value.value == *part).map(|value| value.name.as_str()))
        .collect();
    Some(if names.is_empty() { format!("{} models", parts.len()) } else { names.join(", ") })
}
