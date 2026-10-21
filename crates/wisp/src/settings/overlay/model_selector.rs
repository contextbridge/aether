use super::{KeyHint, SettingsChange, value_match_key};
use crate::surfaces::input::UiOutput;
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::input::{Nav, UiEvent};
use crate::view::reasoning_bar::{reasoning_bar, reasoning_column_width, reasoning_label_width};
use crate::view::selection::Direction;
use crate::view::wrap::truncate_to_width;
use acp_utils::config_option_id::ConfigOptionId;
use client_core::session_config_view::LocalConfigValue;
use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, StatefulWidget, Widget};
use std::collections::BTreeSet;
use utils::ReasoningEffort;

/// Multi-select pane for models, with a reasoning-effort control that follows
/// whichever model is focused.
pub(super) struct ModelSelector {
    config_id: String,
    items: FilterableList<ModelSelectorRow>,
    selected_models: BTreeSet<String>,
    original_models: BTreeSet<String>,
    reasoning_effort: Option<ReasoningEffort>,
    original_reasoning_effort: Option<ReasoningEffort>,
    validation_error: Option<String>,
}

#[derive(Clone, Debug)]
pub(super) enum ModelSelectorRow {
    Provider { label: String, match_key: String },
    Spacer { match_key: String },
    Model { value: LocalConfigValue, match_key: String },
}

impl ModelSelectorRow {
    fn model(&self) -> Option<&LocalConfigValue> {
        match self {
            Self::Model { value, .. } => Some(value),
            _ => None,
        }
    }

    fn is_selectable(&self) -> bool {
        self.model().is_some_and(|value| !value.is_disabled)
    }

    fn match_key(&self) -> String {
        match self {
            Self::Provider { match_key, .. } | Self::Spacer { match_key } | Self::Model { match_key, .. } => {
                match_key.clone()
            }
        }
    }
}

/// Header rows above the model list: search, current selection, and a blank
/// separator before the first provider heading.
const HEADER_ROWS: u16 = 3;
const CHECKBOX_WIDTH: usize = 6;
const CAPABILITY_WIDTH: usize = 11;
const STATUS_WIDTH: usize = 18;

impl ModelSelector {
    pub(super) fn new(
        config_id: String,
        items: Vec<LocalConfigValue>,
        current_selection: &str,
        current_reasoning_effort: Option<&str>,
    ) -> Self {
        let selected_models: BTreeSet<String> = current_selection
            .split(',')
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();
        let reasoning = current_reasoning_effort.and_then(|effort| effort.parse().ok());

        let rows = build_rows(items);
        let mut items = FilterableList::new(rows, ModelSelectorRow::match_key)
            .selectable(ModelSelectorRow::is_selectable)
            .preserve_order()
            .clamped();
        if let Some(index) = items
            .entries()
            .iter()
            .position(|row| row.model().is_some_and(|value| selected_models.contains(&value.value)))
        {
            items.select_index(index);
        }

        Self {
            config_id,
            items,
            original_models: selected_models.clone(),
            selected_models,
            reasoning_effort: reasoning,
            original_reasoning_effort: reasoning,
            validation_error: None,
        }
    }

    fn toggle_focused(&mut self) {
        let Some(value) = self.items.selected_entry().and_then(ModelSelectorRow::model) else {
            return;
        };
        let value = value.value.clone();
        if !self.selected_models.remove(&value) {
            self.selected_models.insert(value);
        }
    }

    fn cycle_reasoning(&mut self, direction: Direction) {
        let levels = self.focused_reasoning_levels().unwrap_or_default();
        self.validation_error = None;
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
        if !effort.is_enabled() {
            return;
        }
        self.reasoning_effort = self
            .focused_reasoning_levels()
            .filter(|levels| levels.iter().any(|effort| effort.is_enabled()))
            .map(|levels| effort.clamp_to(&levels));
    }

    fn focused_reasoning_levels(&self) -> Option<Vec<ReasoningEffort>> {
        self.items
            .selected_entry()
            .and_then(ModelSelectorRow::model)
            .filter(|value| !value.meta.reasoning_levels.is_empty())
            .map(|value| value.meta.reasoning_levels.clone())
    }
}

impl ModelSelector {
    /// Enter toggles the focused model, and Tab cycles reasoning effort, so
    /// those never reach the shared picker navigation.
    pub(crate) fn on_ui_event(&mut self, event: &UiEvent) -> Vec<UiOutput> {
        if let UiEvent::Key(key) = event {
            match key.code {
                KeyCode::Tab => {
                    self.cycle_reasoning(Direction::Forward);
                    return Vec::new();
                }
                KeyCode::BackTab => {
                    self.cycle_reasoning(Direction::Backward);
                    return Vec::new();
                }
                KeyCode::Enter => {
                    self.toggle_focused();
                    return Vec::new();
                }
                _ => {}
            }
        }
        match self.items.on_nav_event(event) {
            Nav::Clicked => {
                self.clamp_reasoning_to_focused();
                self.toggle_focused();
                Vec::new()
            }
            Nav::Moved => {
                self.clamp_reasoning_to_focused();
                Vec::new()
            }
            Nav::Close | Nav::Activate | Nav::Unhandled => Vec::new(),
        }
    }
}

impl ModelSelector {
    /// Model changes are batched and committed when the pane closes, so
    /// toggling several models costs one round-trip instead of one each.
    fn pending_changes(&self) -> Vec<SettingsChange> {
        let models_changed = !self.selected_models.is_empty() && self.selected_models != self.original_models;
        if !models_changed {
            return Vec::new();
        }

        let reset_disabled = self.original_reasoning_effort == Some(ReasoningEffort::Disabled)
            && self.reasoning_effort != Some(ReasoningEffort::Disabled);
        let mut changes = Vec::new();
        if reset_disabled {
            changes.push(SettingsChange {
                config_id: ConfigOptionId::ReasoningEffort.as_str().to_string(),
                new_value: ReasoningEffort::Default.as_str().to_string(),
            });
        }
        changes.push(SettingsChange {
            config_id: self.config_id.clone(),
            new_value: self.selected_models.iter().cloned().collect::<Vec<_>>().join(","),
        });
        if self.reasoning_effort != self.original_reasoning_effort
            && !(reset_disabled && self.reasoning_effort.unwrap_or_default() == ReasoningEffort::Default)
        {
            changes.push(SettingsChange {
                config_id: ConfigOptionId::ReasoningEffort.as_str().to_string(),
                new_value: ReasoningEffort::config_str(self.reasoning_effort).to_string(),
            });
        }
        changes
    }
}

impl ModelSelector {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Min(0)]).areas(area);
        let reasoning_width = self
            .items
            .entries()
            .iter()
            .filter_map(ModelSelectorRow::model)
            .map(|value| reasoning_column_width(&value.meta.reasoning_levels))
            .max()
            .unwrap_or(0);
        let name_width = usize::from(list_area.width)
            .saturating_sub(CHECKBOX_WIDTH + CAPABILITY_WIDTH + STATUS_WIDTH + reasoning_width + 1)
            .max(12);
        let selected = &self.selected_models;
        let selected_names = self
            .items
            .entries()
            .iter()
            .filter_map(ModelSelectorRow::model)
            .filter(|item| selected.contains(&item.value))
            .map(model_label)
            .collect::<Vec<_>>()
            .join(", ");
        let header = vec![
            labeled("Model search", self.items.query(), theme),
            labeled("Selected", &selected_names, theme),
            Line::default(),
        ];
        Paragraph::new(header).render(header_area, buf);

        let focused_value =
            self.items.selected_entry().and_then(ModelSelectorRow::model).map(|value| value.value.clone());
        let reasoning_effort = self.reasoning_effort;
        let (view, selection) = self.items.view(theme, |row| match row {
            ModelSelectorRow::Spacer { .. } => Line::default(),
            ModelSelectorRow::Provider { label, .. } => Line::styled(format!(" {label}"), theme.heading),
            ModelSelectorRow::Model { value, .. } => {
                let is_selected = selected.contains(&value.value);
                let is_focused = focused_value.as_deref() == Some(value.value.as_str());
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
                let label = truncate_to_width(model_label(value), name_width.saturating_sub(1));
                let reasoning = if is_focused && !value.meta.reasoning_levels.is_empty() {
                    let levels = &value.meta.reasoning_levels;
                    reasoning_bar(reasoning_effort, levels, reasoning_label_width(levels))
                } else {
                    String::new()
                };
                let capabilities = match (value.meta.supports_image, value.meta.supports_audio) {
                    (true, true) => "img  audio",
                    (true, false) => "img",
                    (false, true) => "audio",
                    (false, false) => "",
                };
                let status = truncate_to_width(status, STATUS_WIDTH);
                Line::styled(
                    format!(
                        "  {checkbox}{label:name_width$}{reasoning:reasoning_width$}{capabilities:CAPABILITY_WIDTH$}{status}"
                    ),
                    if value.is_disabled {
                        Style::from(theme.text_secondary)
                    } else {
                        Style::from(theme.text_primary)
                    },
                )
            }
        });
        StatefulWidget::render(
            view.pane(" (no matches found)").scrollbar().highlight_horizontal_padding(MODAL_HORIZONTAL_PADDING),
            list_area,
            buf,
            selection,
        );
        if let Some(error) = &self.validation_error {
            Paragraph::new(error.as_str())
                .style(theme.warning)
                .render(Rect::new(area.x, area.y + 2, area.width, 1), buf);
        }
    }
}

fn build_rows(items: Vec<LocalConfigValue>) -> Vec<ModelSelectorRow> {
    let mut groups: Vec<(String, String, Vec<LocalConfigValue>)> = Vec::new();
    for item in items {
        let (key, label) = provider(&item);
        if let Some((_, _, models)) = groups.iter_mut().find(|(existing, _, _)| *existing == key) {
            models.push(item);
        } else {
            groups.push((key, label, vec![item]));
        }
    }

    let mut rows = Vec::new();
    for (position, (_, label, models)) in groups.into_iter().enumerate() {
        let models_match_key = models.iter().map(value_match_key).collect::<Vec<_>>().join(" ");
        let match_key = format!("{label} {models_match_key}");
        if position > 0 {
            rows.push(ModelSelectorRow::Spacer { match_key: match_key.clone() });
        }
        rows.push(ModelSelectorRow::Provider { label: label.clone(), match_key });
        rows.extend(
            models.into_iter().map(|value| ModelSelectorRow::Model {
                match_key: format!("{label} {}", value_match_key(&value)),
                value,
            }),
        );
    }
    rows
}

/// Fully-unavailable providers arrive from the agent collapsed into one row
/// per provider, whose value carries this prefix.
const UNAVAILABLE_VALUE_PREFIX: &str = "__unavailable:";

fn provider(value: &LocalConfigValue) -> (String, String) {
    if let Some(group) = &value.group {
        return (group.to_lowercase(), group.clone());
    }
    if value.value.starts_with(UNAVAILABLE_VALUE_PREFIX) {
        return ("unavailable".to_string(), "Unavailable".to_string());
    }
    if let Some((label, _)) = name_parts(&value.name) {
        return (label.to_lowercase(), label.to_string());
    }

    let key = value.value.split_once(':').map_or("Other", |(provider, _)| provider);
    let mut chars = key.chars();
    let first = chars.next().map(char::to_uppercase).into_iter().flatten().collect::<String>();
    let label = format!("{first}{}", chars.as_str().to_lowercase());
    (key.to_lowercase(), label)
}

fn model_label(value: &LocalConfigValue) -> &str {
    name_parts(&value.name).map_or(value.name.as_str(), |(_, model)| model)
}

/// The provider and model halves of an agent-provided display name, whether it
/// is written "Anthropic / Opus" or `DeepSeek: DeepSeek-V3`.
fn name_parts(name: &str) -> Option<(&str, &str)> {
    name.split_once(" / ").or_else(|| name.split_once(": "))
}

fn labeled(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label}: "), theme.text_secondary),
        Span::styled(value.to_string(), theme.text_primary),
    ])
}

impl ModelSelector {
    pub(crate) fn take_changes(&mut self) -> Option<Vec<UiOutput>> {
        let invalid = self.reasoning_effort == Some(ReasoningEffort::Disabled)
            && self.items.entries().iter().filter_map(ModelSelectorRow::model).any(|value| {
                self.selected_models.contains(&value.value)
                    && !value.meta.reasoning_levels.contains(&ReasoningEffort::Disabled)
            });
        if invalid {
            self.validation_error = Some("Cannot disable reasoning; choose Default or a supported level".to_string());
            return None;
        }
        Some(self.pending_changes().into_iter().map(UiOutput::ConfigChange).collect())
    }

    pub(crate) fn footer(&self) -> Vec<KeyHint> {
        vec![
            ("Enter", "toggle".into()),
            ("Tab", format!("effort: {}", ReasoningEffort::config_str(self.reasoning_effort)).into()),
            ("Esc", "done".into()),
        ]
    }
}
