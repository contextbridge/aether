use super::{PaneKind, SettingsChange, SettingsMenuEntry, SettingsMenuValue};
use crate::session::session_config_view::{LocalConfigKind, LocalConfigOption};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::list_view::ListView;
use crate::view::selection::{Direction, SelectionState};
use acp_utils::config_option_id::ConfigOptionId;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::StatefulWidget;

pub(super) struct SettingsMenu {
    pub(super) rows: Vec<MenuRow>,
    pub(super) selection: SelectionState,
}

pub(super) struct SettingsMenuView<'a> {
    rows: &'a [MenuRow],
    theme: &'a Theme,
}

impl<'a> SettingsMenuView<'a> {
    pub(super) fn new(rows: &'a [MenuRow], theme: &'a Theme) -> Self {
        Self { rows, theme }
    }
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

    pub(super) fn step(&mut self, direction: Direction) {
        self.selection.step(self.rows.len(), direction, |_| true);
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
        let Self { rows, selection, .. } = self;
        StatefulWidget::render(SettingsMenuView::new(rows, theme), area, buffer, selection);
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

impl StatefulWidget for SettingsMenuView<'_> {
    type State = SelectionState;

    fn render(self, area: Rect, buffer: &mut Buffer, state: &mut Self::State) {
        let rows = self
            .rows
            .iter()
            .map(|row| Line::styled(format!(" {}", row.label()), Style::new().fg(self.theme.text_primary)))
            .collect();
        let view = ListView::new(rows, self.theme)
            .pane(" (no settings options)")
            .highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING);
        StatefulWidget::render(view, area, buffer, state);
    }
}

fn menu_entry(option: &LocalConfigOption) -> Option<SettingsMenuEntry> {
    let LocalConfigKind::Select { current_value, values: option_values, multi_select } = &option.kind else {
        return None;
    };
    if option_values.is_empty() {
        return None;
    }

    let current_value_index = option_values.iter().position(|value| value.value == *current_value).unwrap_or(0);
    let values: Vec<SettingsMenuValue> = option_values
        .iter()
        .map(|value| SettingsMenuValue {
            value: value.value.clone(),
            name: value.name.clone(),
            group: value.group.clone(),
            is_disabled: value.is_disabled,
            description: value.description.clone(),
            meta: value.meta.clone(),
        })
        .collect();

    Some(SettingsMenuEntry {
        config_id: option.id.clone(),
        title: option.name.clone(),
        current_value_index,
        current_raw_value: current_value.clone(),
        display_name: (*multi_select).then(|| combined_display_name(current_value, &values)).flatten(),
        values,
        multi_select: *multi_select,
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
