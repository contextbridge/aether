use super::{PaneKind, SettingsChange, SettingsMenuEntry};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::list_view::ListView;
use crate::view::selection::SelectionState;
use acp_utils::config_option_id::ConfigOptionId;
use client_core::session_config_view::LocalConfigOption;
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
    /// The `title: value` line drawn for this row, with `summary` describing a
    /// pane's live data.
    fn label(&self, summary: impl Fn(PaneKind) -> String) -> String {
        match self {
            Self::Select(entry) => {
                let select = &entry.select;
                let current = select.selected_names().unwrap_or_else(|| match select.current_values().count() {
                    count if count > 1 => format!("{count} models"),
                    _ => select.values.first().map_or_else(|| "?".to_owned(), |value| value.name.clone()),
                });
                format!("{}: {current}", entry.title)
            }
            Self::Pane(kind) => format!("{}: {}", kind.title(), summary(*kind)),
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
            entry.select.current_value.clone_from(&change.new_value);
        }
    }

    pub(super) fn render(
        &mut self,
        area: Rect,
        buffer: &mut Buffer,
        theme: &Theme,
        summary: impl Fn(PaneKind) -> String,
    ) {
        let rows =
            self.rows.iter().map(|row| Line::styled(format!(" {}", row.label(&summary)), theme.text_primary)).collect();
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
    let select = option.select().filter(|select| !select.values.is_empty())?;
    Some(SettingsMenuEntry {
        config_id: option.id.clone(),
        title: option.name.clone(),
        select: select.clone(),
        local: false,
    })
}
