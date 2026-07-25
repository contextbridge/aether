use super::{PaneOutcome, SettingsChange, SettingsMenuValue, SettingsPane};
use crate::filterable_list::FilterableList;
use crate::selection::Direction;
use crate::theme::Theme;
use crate::wrap::truncate_to_width;
use acp_utils::config_option_id::ConfigOptionId;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{ListItem, Paragraph, Widget};
use std::collections::BTreeSet;
use utils::ReasoningEffort;

/// Multi-select pane for models, with a reasoning-effort control that follows
/// whichever model is focused.
pub(super) struct ModelSelector {
    config_id: String,
    items: FilterableList<SettingsMenuValue>,
    selected_models: BTreeSet<String>,
    original_models: BTreeSet<String>,
    reasoning_effort: Option<ReasoningEffort>,
    original_reasoning_effort: Option<ReasoningEffort>,
}

/// Header rows above the model list: search, current selection, column titles.
const HEADER_ROWS: u16 = 3;
const CHECKBOX_WIDTH: usize = 4;
const CAPABILITY_WIDTH: usize = 11;
const STATUS_WIDTH: usize = 18;

impl ModelSelector {
    pub(super) fn new(
        config_id: String,
        items: Vec<SettingsMenuValue>,
        current_selection: &str,
        current_reasoning_effort: Option<&str>,
    ) -> Self {
        let selected_models: BTreeSet<String> = current_selection
            .split(',')
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        let reasoning = current_reasoning_effort.and_then(|effort| effort.parse().ok());

        let mut items = FilterableList::new(items, |value| format!("{} {}", value.name, value.value));
        // Focus a model that is already chosen, else the first selectable one.
        let initial = items
            .entries()
            .iter()
            .position(|value| !value.is_disabled && selected_models.contains(&value.value))
            .or_else(|| items.entries().iter().position(|value| !value.is_disabled));
        if let Some(index) = initial {
            items.select_index(index);
        }

        Self {
            config_id,
            items,
            original_models: selected_models.clone(),
            selected_models,
            reasoning_effort: reasoning,
            original_reasoning_effort: reasoning,
        }
    }

    fn toggle_focused(&mut self) {
        let Some(value) = self.items.selected_entry().filter(|value| !value.is_disabled) else {
            return;
        };
        let value = value.value.clone();
        if !self.selected_models.remove(&value) {
            self.selected_models.insert(value);
        }
    }

    fn cycle_reasoning(&mut self, direction: Direction) {
        let Some(levels) = self.focused_reasoning_levels() else {
            return;
        };
        self.reasoning_effort = match direction {
            Direction::Forward => ReasoningEffort::cycle_within(self.reasoning_effort, &levels),
            Direction::Backward => ReasoningEffort::cycle_within_back(self.reasoning_effort, &levels),
        };
    }

    /// Keeps the effort within what the focused model actually supports, so
    /// moving between models never leaves an impossible setting showing.
    fn clamp_reasoning_to_focused(&mut self) {
        let Some(effort) = self.reasoning_effort else {
            return;
        };
        self.reasoning_effort = self.focused_reasoning_levels().map(|levels| effort.clamp_to(&levels));
    }

    fn focused_reasoning_levels(&self) -> Option<Vec<ReasoningEffort>> {
        self.items
            .selected_entry()
            .filter(|value| !value.is_disabled && !value.meta.reasoning_levels.is_empty())
            .map(|value| value.meta.reasoning_levels.clone())
    }

    fn reasoning_label(&self) -> &'static str {
        ReasoningEffort::config_str(self.reasoning_effort)
    }

    fn selected_names(&self) -> String {
        self.items
            .entries()
            .iter()
            .filter(|item| self.selected_models.contains(&item.value))
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl SettingsPane for ModelSelector {
    fn on_key(&mut self, key: KeyEvent) -> PaneOutcome {
        match key.code {
            KeyCode::Up => self.scroll(Direction::Backward),
            KeyCode::Down => self.scroll(Direction::Forward),
            KeyCode::Tab => self.cycle_reasoning(Direction::Forward),
            KeyCode::BackTab => self.cycle_reasoning(Direction::Backward),
            KeyCode::Enter | KeyCode::Char(' ') => self.toggle_focused(),
            KeyCode::Backspace => self.items.pop_query_char(),
            KeyCode::Char(character) if !character.is_control() => self.items.push_query_char(character),
            _ => {}
        }
        PaneOutcome::default()
    }

    fn click(&mut self, row: usize, _height: usize) -> PaneOutcome {
        if let Some(item_row) = row.checked_sub(usize::from(HEADER_ROWS)) {
            self.items.select_row(item_row);
            self.toggle_focused();
        }
        PaneOutcome::default()
    }

    /// Scanning a long model list stops at either end rather than wrapping.
    fn scroll(&mut self, direction: Direction) {
        self.items.step_clamped(direction, |value| !value.is_disabled);
        self.clamp_reasoning_to_focused();
    }

    /// Model changes are batched and committed when the pane closes, so
    /// toggling several models costs one round-trip instead of one each.
    fn take_changes(&mut self) -> Vec<SettingsChange> {
        let mut changes = Vec::new();
        if !self.selected_models.is_empty() && self.selected_models != self.original_models {
            changes.push(SettingsChange {
                config_id: self.config_id.clone(),
                new_value: self.selected_models.iter().cloned().collect::<Vec<_>>().join(","),
            });
        }
        if self.reasoning_effort != self.original_reasoning_effort {
            changes.push(SettingsChange {
                config_id: ConfigOptionId::ReasoningEffort.as_str().to_string(),
                new_value: ReasoningEffort::config_str(self.reasoning_effort).to_string(),
            });
        }
        changes
    }

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Min(0)]).areas(area);
        let name_width = name_column_width(list_area.width);
        let header = format!(
            " Model search: {}\n Selected: {}\n {blank:CHECKBOX_WIDTH$}{model:name_width$}{capabilities:CAPABILITY_WIDTH$}Status",
            self.items.query(),
            self.selected_names(),
            blank = "",
            model = "Model",
            capabilities = "Capabilities",
            CHECKBOX_WIDTH = CHECKBOX_WIDTH,
            CAPABILITY_WIDTH = CAPABILITY_WIDTH,
        );
        Paragraph::new(header).style(Style::new().fg(theme.text_secondary)).render(header_area, buf);

        let selected = &self.selected_models;
        self.items
            .view(theme, " (no matches found)", |value| {
                let is_selected = selected.contains(&value.value);
                let status = if value.is_disabled {
                    value
                        .description
                        .as_deref()
                        .and_then(|description| description.strip_prefix("Unavailable: "))
                        .unwrap_or("unavailable")
                } else if is_selected {
                    "selected"
                } else {
                    ""
                };
                let checkbox = if is_selected { "[x] " } else { "[ ] " };
                let label = truncate_to_width(model_label(&value.name), name_width.saturating_sub(1));
                let capabilities = capability_tags(value.meta.supports_image, value.meta.supports_audio);
                let status = truncate_to_width(status, STATUS_WIDTH);
                ListItem::new(format!(" {checkbox}{label:name_width$}{capabilities:CAPABILITY_WIDTH$}{status}")).style(
                    if value.is_disabled {
                        Style::new().fg(theme.text_secondary)
                    } else {
                        Style::new().fg(theme.text_primary)
                    },
                )
            })
            .highlight_style(Style::new().fg(theme.background).bg(theme.text_primary))
            .scrollbar()
            .render(list_area, buf);
    }

    fn footer(&self) -> String {
        format!("[Space/Enter] Toggle  [Tab] Effort: {}  [Esc] Done", self.reasoning_label())
    }
}

/// Width left for the model name once the fixed columns are accounted for.
fn name_column_width(total: u16) -> usize {
    usize::from(total).saturating_sub(CHECKBOX_WIDTH + CAPABILITY_WIDTH + STATUS_WIDTH + 1).max(12)
}

/// Strips the "Provider / " prefix so the provider is not repeated on every row.
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
