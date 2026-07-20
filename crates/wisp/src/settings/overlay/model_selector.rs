use super::{KeyHint, SettingsChange, SettingsMenuValue, message_for_change, value_match_key};
use crate::surfaces::input::{Nav, SettingsOutput, UiEvent, is_press};
use crate::surfaces::modal::frame::MODAL_HORIZONTAL_PADDING;
use crate::theme::Theme;
use crate::view::filterable_list::FilterableList;
use crate::view::reasoning_bar::{reasoning_bar, reasoning_column_width, reasoning_label_width};
use crate::view::selection::Direction;
use crate::view::wrap::truncate_to_width;
use acp_utils::config_option_id::ConfigOptionId;
use crossterm::event::KeyCode;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Position, Rect};
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
}

#[derive(Clone, Debug)]
pub(super) enum ModelSelectorRow {
    Provider { label: String, match_key: String },
    Spacer { match_key: String },
    Model { value: SettingsMenuValue, match_key: String },
}

impl ModelSelectorRow {
    fn model(&self) -> Option<&SettingsMenuValue> {
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

pub(super) struct ModelSelectorView<'a> {
    selected: &'a BTreeSet<String>,
    /// The focused model's value, so only its row carries the reasoning bar.
    focused_value: Option<&'a str>,
    reasoning_effort: Option<ReasoningEffort>,
    theme: &'a Theme,
}

impl<'a> ModelSelectorView<'a> {
    pub(super) fn new(
        selected: &'a BTreeSet<String>,
        focused_value: Option<&'a str>,
        reasoning_effort: Option<ReasoningEffort>,
        theme: &'a Theme,
    ) -> Self {
        Self { selected, focused_value, reasoning_effort, theme }
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

        let rows = build_rows(items);
        let mut items = FilterableList::new(rows, ModelSelectorRow::match_key)
            .selectable(ModelSelectorRow::is_selectable)
            .preserve_order()
            .clamped();
        let initial = items
            .entries()
            .iter()
            .position(|row| {
                row.model().is_some_and(|value| !value.is_disabled && selected_models.contains(&value.value))
            })
            .or_else(|| items.entries().iter().position(ModelSelectorRow::is_selectable));
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
        let Some(value) =
            self.items.selected_entry().and_then(ModelSelectorRow::model).filter(|value| !value.is_disabled)
        else {
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
            Direction::Backward => cycle_reasoning_back(self.reasoning_effort, &levels),
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
            .and_then(ModelSelectorRow::model)
            .filter(|value| !value.is_disabled && !value.meta.reasoning_levels.is_empty())
            .map(|value| value.meta.reasoning_levels.clone())
    }
}

impl ModelSelector {
    /// Enter toggles the focused model, and Tab cycles reasoning effort, so
    /// those never reach the shared picker navigation.
    #[allow(clippy::needless_pass_by_value)]
    pub(crate) fn on_ui_event(&mut self, event: UiEvent) -> Vec<SettingsOutput> {
        if let UiEvent::Key(key) = &event
            && is_press(*key)
        {
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
        match self.items.on_nav_event(&event) {
            Nav::Close => vec![SettingsOutput::Close],
            Nav::Clicked => {
                self.toggle_focused();
                Vec::new()
            }
            Nav::Moved => {
                self.clamp_reasoning_to_focused();
                Vec::new()
            }
            Nav::Activate | Nav::Unhandled => Vec::new(),
        }
    }
}

impl ModelSelector {
    pub(super) fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) -> Option<Position> {
        let focused_value =
            self.items.selected_entry().and_then(ModelSelectorRow::model).map(|value| value.value.clone());
        let view =
            ModelSelectorView::new(&self.selected_models, focused_value.as_deref(), self.reasoning_effort, theme);
        StatefulWidget::render(view, area, buf, &mut self.items);
        None
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

        let mut changes = vec![SettingsChange {
            config_id: self.config_id.clone(),
            new_value: self.selected_models.iter().cloned().collect::<Vec<_>>().join(","),
        }];
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
    type State = FilterableList<ModelSelectorRow>;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State) {
        let [header_area, list_area] =
            Layout::vertical([Constraint::Length(HEADER_ROWS), Constraint::Min(0)]).areas(area);
        let reasoning_width = state
            .entries()
            .iter()
            .filter_map(ModelSelectorRow::model)
            .map(|value| reasoning_column_width(&value.meta.reasoning_levels))
            .max()
            .unwrap_or(0);
        let name_width = usize::from(list_area.width)
            .saturating_sub(CHECKBOX_WIDTH + CAPABILITY_WIDTH + STATUS_WIDTH + reasoning_width + 1)
            .max(12);
        let selected_names = state
            .entries()
            .iter()
            .filter_map(ModelSelectorRow::model)
            .filter(|item| self.selected.contains(&item.value))
            .map(model_label)
            .collect::<Vec<_>>()
            .join(", ");
        let header = vec![
            labeled("Model search", state.query(), self.theme),
            labeled("Selected", &selected_names, self.theme),
            Line::default(),
        ];
        Paragraph::new(header).render(header_area, buf);

        let selected = self.selected;
        let focused_value = self.focused_value;
        let reasoning_effort = self.reasoning_effort;
        let (view, selection) = state.view(self.theme, |row| match row {
            ModelSelectorRow::Spacer { .. } => Line::default(),
            ModelSelectorRow::Provider { label, .. } => {
                Line::styled(format!(" {label}"), Style::new().fg(self.theme.heading))
            }
            ModelSelectorRow::Model { value, .. } => {
                let is_selected = selected.contains(&value.value);
                let is_focused = focused_value == Some(value.value.as_str());
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
                        Style::new().fg(self.theme.text_secondary)
                    } else {
                        Style::new().fg(self.theme.text_primary)
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
    }
}

fn build_rows(items: Vec<SettingsMenuValue>) -> Vec<ModelSelectorRow> {
    let mut groups: Vec<(String, String, Vec<SettingsMenuValue>)> = Vec::new();
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

fn cycle_reasoning_back(current: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> Option<ReasoningEffort> {
    match current {
        None => levels.last().copied(),
        Some(effort) => levels
            .iter()
            .position(|&level| level == effort)
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| levels.get(index))
            .copied(),
    }
}

fn provider(value: &SettingsMenuValue) -> (String, String) {
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

fn model_label(value: &SettingsMenuValue) -> &str {
    name_parts(&value.name).map_or(value.name.as_str(), |(_, model)| model)
}

/// The provider and model halves of an agent-provided display name, whether it
/// is written "Anthropic / Opus" or `DeepSeek: DeepSeek-V3`.
fn name_parts(name: &str) -> Option<(&str, &str)> {
    name.split_once(" / ").or_else(|| name.split_once(": "))
}

fn labeled(label: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!(" {label}: "), Style::new().fg(theme.text_secondary)),
        Span::styled(value.to_string(), Style::new().fg(theme.text_primary)),
    ])
}

impl ModelSelector {
    pub(crate) fn take_changes(&mut self) -> Vec<SettingsOutput> {
        self.pending_changes().iter().map(message_for_change).collect()
    }

    pub(crate) fn footer(&self) -> Vec<KeyHint> {
        vec![
            ("Enter", "toggle".into()),
            ("Tab", format!("effort: {}", ReasoningEffort::config_str(self.reasoning_effort)).into()),
            ("Esc", "done".into()),
        ]
    }
}
