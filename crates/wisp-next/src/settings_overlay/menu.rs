use super::{PaneKind, SettingsChange, SettingsMenuEntry, SettingsMenuValue};
use crate::list_view::ListView;
use crate::selection::{Direction, SelectionState};
use crate::session_config_view::{as_select, select_values};
use crate::theme::Theme;
use acp_utils::config_meta::{ConfigOptionMeta, SelectOptionMeta};
use acp_utils::config_option_id::ConfigOptionId;
use agent_client_protocol::schema::SessionConfigOption;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Widget;

pub(super) struct SettingsMenu {
    pub(super) rows: Vec<MenuRow>,
    pub(super) selection: SelectionState,
}

/// One row of the settings menu.
#[derive(Clone)]
pub(super) enum MenuRow {
    /// A config option, opening a picker over its values.
    Select(SettingsMenuEntry),
    /// A row that opens a pane rather than picking a value. Its "value" is just
    /// the summary line shown beside the title.
    Pane { kind: PaneKind, summary: String },
}

impl MenuRow {
    /// The `title: value` line drawn for this row.
    fn label(&self) -> String {
        match self {
            Self::Select(entry) => {
                let current = entry
                    .display_name
                    .as_deref()
                    .or_else(|| entry.values.get(entry.current_value_index).map(|value| value.name.as_str()))
                    .unwrap_or("?");
                format!("{}: {current}", entry.title)
            }
            Self::Pane { kind, summary } => format!("{}: {summary}", kind.title()),
        }
    }

    /// Whether this row came from the agent's config schema, and so is replaced
    /// wholesale when that schema is pushed again.
    fn is_agent_select(&self) -> bool {
        matches!(self, Self::Select(entry) if !entry.local)
    }
}

impl SettingsMenu {
    pub(super) fn from_config_options(options: &[SessionConfigOption]) -> Self {
        let rows: Vec<MenuRow> = options
            .iter()
            .filter(|option| option.id.0.as_ref() != ConfigOptionId::ReasoningEffort.as_str())
            .filter_map(|option| menu_entry(option).map(MenuRow::Select))
            .collect();
        let selection = SelectionState::new(rows.len());
        Self { rows, selection }
    }

    pub(super) fn step(&mut self, direction: Direction) {
        self.selection.step(self.rows.len(), direction, |_| true);
    }

    pub(super) fn selected_row(&self) -> Option<&MenuRow> {
        self.selection.selected().and_then(|selected| self.rows.get(selected))
    }

    /// Adds rows that are not part of the agent's schema, above the ones that are.
    pub(super) fn add_local_entries(&mut self, entries: Vec<SettingsMenuEntry>) {
        self.rows.splice(0..0, entries.into_iter().map(MenuRow::Select));
        self.selection.clamp(self.rows.len());
    }

    /// Replaces the agent-provided rows, keeping local and pane rows.
    pub(super) fn update_options(&mut self, options: &[SessionConfigOption]) {
        let preserved: Vec<MenuRow> = self.rows.iter().filter(|row| !row.is_agent_select()).cloned().collect();
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
            if let Some(index) = entry.values.iter().position(|value| value.value == change.new_value) {
                entry.current_value_index = index;
            }
        }
    }

    /// Selects the row drawn at terminal `row`, reporting whether one was hit.
    pub(super) fn click_at(&mut self, row: u16) -> bool {
        self.selection.select_at(row, self.rows.len())
    }

    pub(super) fn render(&mut self, area: Rect, buffer: &mut Buffer, theme: &Theme) {
        let rows: Vec<Line<'static>> = self
            .rows
            .iter()
            .map(|row| Line::styled(format!(" {}", row.label()), Style::new().fg(theme.text_primary)))
            .collect();
        ListView::new(rows, &mut self.selection, theme).pane(" (no settings options)").render(area, buffer);
    }

    /// Adds or refreshes the row that opens `kind`'s pane.
    pub(super) fn upsert_pane_row(&mut self, kind: PaneKind, summary: &str) {
        let row = MenuRow::Pane { kind, summary: summary.to_string() };
        let existing =
            self.rows.iter().position(|row| matches!(row, MenuRow::Pane { kind: existing, .. } if *existing == kind));
        match existing {
            Some(index) => self.rows[index] = row,
            None => self.rows.push(row),
        }
        self.selection.clamp(self.rows.len());
    }
}

/// The menu entry for one select-kind config option, or nothing when it offers
/// no values to pick between.
fn menu_entry(option: &SessionConfigOption) -> Option<SettingsMenuEntry> {
    let select = as_select(option)?;
    let flat_options = select_values(select);
    if flat_options.is_empty() {
        return None;
    }

    let current_value_index = flat_options.iter().position(|value| value.value == select.current_value).unwrap_or(0);
    let values: Vec<SettingsMenuValue> = flat_options
        .into_iter()
        .map(|value| SettingsMenuValue {
            value: value.value.0.to_string(),
            name: value.name.clone(),
            is_disabled: value.description.as_deref().is_some_and(|text| text.starts_with("Unavailable:")),
            description: value.description.clone(),
            meta: SelectOptionMeta::from_meta(value.meta.as_ref()),
        })
        .collect();
    let multi_select = ConfigOptionMeta::from_meta(option.meta.as_ref()).multi_select;

    Some(SettingsMenuEntry {
        config_id: option.id.0.to_string(),
        title: option.name.clone(),
        current_value_index,
        current_raw_value: select.current_value.0.to_string(),
        display_name: multi_select.then(|| combined_display_name(&select.current_value.0, &values)).flatten(),
        values,
        multi_select,
        local: false,
    })
}

/// The label for a multi-select whose value names several options at once,
/// falling back to a count when none of them are known.
fn combined_display_name(current: &str, values: &[SettingsMenuValue]) -> Option<String> {
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
