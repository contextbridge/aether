use super::{KeyHint, SettingsChange, SettingsMenuValue, SettingsPaneBehavior, message_for_change};
use crate::components::filterable_list::FilterableList;
use crate::components::selection::Direction;
use crate::components::theme::Theme;
use crate::components::wrap::truncate_to_width;
use crate::surfaces::surface::{Action, Surface, SurfaceList, is_composed_char};
use acp_utils::config_option_id::ConfigOptionId;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};
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

pub(super) struct ModelSelectorView<'a> {
    selected: &'a BTreeSet<String>,
    theme: &'a Theme,
}

impl<'a> ModelSelectorView<'a> {
    pub(super) fn new(selected: &'a BTreeSet<String>, theme: &'a Theme) -> Self {
        Self { selected, theme }
    }
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

        // Scanning a long model list stops at either end rather than wrapping,
        // and never rests on a model the agent cannot offer.
        let mut items = FilterableList::new(items, |value| format!("{} {}", value.name, value.value))
            .selectable(|value| !value.is_disabled)
            .clamped();
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
}

impl Surface for ModelSelector {
    fn on_surface_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        match key.code {
            KeyCode::Tab => self.cycle_reasoning(Direction::Forward),
            KeyCode::BackTab => self.cycle_reasoning(Direction::Backward),
            KeyCode::Enter | KeyCode::Char(' ') if !is_composed_char(key) => self.toggle_focused(),
            _ => return None,
        }
        Some(Vec::new())
    }

    /// Toggling is what Enter does, so it is also what a click does.
    fn activate(&mut self) -> Vec<Action> {
        self.toggle_focused();
        Vec::new()
    }

    fn list(&mut self) -> Option<&mut dyn SurfaceList> {
        Some(&mut self.items)
    }

    fn activates_on_click(&self) -> bool {
        true
    }

    /// The effort control follows whichever model the move focused.
    fn on_selection_changed(&mut self) -> Vec<Action> {
        self.clamp_reasoning_to_focused();
        Vec::new()
    }
}

impl ModelSelector {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        StatefulWidget::render(ModelSelectorView::new(&self.selected_models, theme), area, buf, &mut self.items);
        None
    }
}

impl ModelSelector {
    /// Model changes are batched and committed when the pane closes, so
    /// toggling several models costs one round-trip instead of one each.
    fn pending_changes(&self) -> Vec<SettingsChange> {
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
}

impl StatefulWidget for ModelSelectorView<'_> {
    type State = FilterableList<SettingsMenuValue>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Min(0)]).areas(area);
        let name_width = name_column_width(list_area.width);
        let selected_names = state
            .entries()
            .iter()
            .filter(|item| self.selected.contains(&item.value))
            .map(|item| item.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let header = format!(
            " Model search: {}\n Selected: {}\n {blank:CHECKBOX_WIDTH$}{model:name_width$}{capabilities:CAPABILITY_WIDTH$}Status",
            state.query(),
            selected_names,
            blank = "",
            model = "Model",
            capabilities = "Capabilities",
            CHECKBOX_WIDTH = CHECKBOX_WIDTH,
            CAPABILITY_WIDTH = CAPABILITY_WIDTH,
        );
        Paragraph::new(header).style(Style::new().fg(self.theme.text_secondary)).render(header_area, buf);

        let selected = self.selected;
        let (view, selection) = state.view(self.theme, |value| {
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
            Line::styled(
                format!(" {checkbox}{label:name_width$}{capabilities:CAPABILITY_WIDTH$}{status}"),
                if value.is_disabled {
                    Style::new().fg(self.theme.text_secondary)
                } else {
                    Style::new().fg(self.theme.text_primary)
                },
            )
        });
        StatefulWidget::render(view.pane(" (no matches found)").scrollbar(), list_area, buf, selection);
    }
}

impl SettingsPaneBehavior for ModelSelector {
    fn take_changes(&mut self) -> Vec<Action> {
        self.pending_changes().iter().map(message_for_change).collect()
    }

    fn footer(&self) -> Vec<KeyHint> {
        vec![
            ("Space/Enter", "toggle".into()),
            ("Tab", format!("effort: {}", self.reasoning_label()).into()),
            ("Esc", "done".into()),
        ]
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
