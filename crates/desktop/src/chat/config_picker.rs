use crate::fuzzy;
use crate::theme::DesktopTheme;
use crate::ui::tokens::{BAR_HEIGHT, RADIUS_CONTAINER, rem};
use crate::ui::{Icon, IconName, ListItem, Typography, TypographyExt};
use client_core::session_config_view::LocalConfigValue;
use gpui::{
    ClickEvent, Context, Entity, EventEmitter, FocusHandle, KeyBinding, NoAction, ScrollHandle, Subscription, Window,
    div, prelude::*,
};
use gpui_base::Selectable as _;
use gpui_base::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};

/// The values of one of the session's options, such as its models, to choose one from.
///
/// It opens with the current value highlighted. ↑ and ↓ move the highlight past values the agent
/// marks unavailable, Enter or a click chooses, and Escape dismisses. An option with more than
/// [`SEARCH_THRESHOLD`] values offers a search field, focused, that narrows the list as the user
/// types. The list scrolls once it is taller than its cap.
///
/// It is tagged `config-<option id>-picker`, its search field `config-<option id>-search`, and each
/// value `config-<option id>-<value>`.
pub(crate) struct ConfigPicker {
    selector: String,
    name: String,
    values: Vec<LocalConfigValue>,
    current: String,
    search: Option<(Entity<InputState>, Subscription)>,
    focus_handle: FocusHandle,
    /// Whether focus has yet to move into the picker, which happens once it first paints.
    needs_focus: bool,
    /// Indices into `values` of those matching the search, best match first.
    matches: Vec<usize>,
    /// The position in `matches` of the highlighted value.
    highlighted: usize,
    scroll: ScrollHandle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ConfigPickerEvent {
    Chose(String),
    Dismissed,
}

/// Options with more values than this can be searched.
pub(crate) const SEARCH_THRESHOLD: usize = 8;

/// The key context of the picker, in which the arrow keys, Enter and Escape drive the list.
pub(crate) const CONTEXT: &str = "ConfigPicker";

/// Binds the keys that drive the list when the picker has no search field to take them.
pub(crate) fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys([
        // The popover the picker opens in binds space to toggling itself, which would close the
        // picker on every space typed into its search.
        KeyBinding::new("space", NoAction, Some(CONTEXT)),
        KeyBinding::new("up", MoveUp, Some(CONTEXT)),
        KeyBinding::new("down", MoveDown, Some(CONTEXT)),
        KeyBinding::new("enter", Enter { secondary: false, shift: false }, Some(CONTEXT)),
        KeyBinding::new("escape", Escape, Some(CONTEXT)),
    ]);
}

impl ConfigPicker {
    pub(crate) fn new(
        option_id: &str,
        name: String,
        values: Vec<LocalConfigValue>,
        current: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = (values.len() > SEARCH_THRESHOLD).then(|| {
            let style = DesktopTheme::global(cx).editor_style();
            let placeholder = format!("Search {}…", name.to_lowercase());
            let input = cx.new(|cx| {
                let mut input = InputState::new(window, cx).placeholder(placeholder);
                input.set_editor_style(style);
                input
            });
            let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    this.refilter(cx);
                }
            });
            (input, subscription)
        });
        let matches: Vec<usize> = (0..values.len()).collect();
        let highlighted = values.iter().position(|value| value.value == current).unwrap_or(0);
        let scroll = ScrollHandle::new();
        scroll.scroll_to_item(highlighted);
        Self {
            selector: format!("config-{option_id}"),
            name,
            values,
            current,
            search,
            focus_handle: cx.focus_handle(),
            needs_focus: true,
            matches,
            highlighted,
            scroll,
        }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.search.as_ref().map(|(input, _)| input.read(cx).value()).unwrap_or_default();
        self.matches = if query.trim().is_empty() {
            (0..self.values.len()).collect()
        } else {
            let keys = self.values.iter().map(|value| format!("{} {}", value.name, value.value));
            fuzzy::filter(&query, keys)
        };
        self.highlighted = self.next_enabled(0, 1).unwrap_or(0);
        self.scroll.scroll_to_item(self.highlighted);
        cx.notify();
    }

    /// Moves the highlight to the next available value in direction `by`, staying put at either
    /// end of the list.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        if let Some(next) = self.highlighted.checked_add_signed(by).and_then(|start| self.next_enabled(start, by)) {
            self.highlighted = next;
            self.scroll.scroll_to_item(next);
            cx.notify();
        }
    }

    /// The first position from `start`, going in direction `by`, whose value is available.
    fn next_enabled(&self, start: usize, by: isize) -> Option<usize> {
        let mut position = start;
        loop {
            let value = &self.values[*self.matches.get(position)?];
            if !value.is_disabled {
                return Some(position);
            }
            position = position.checked_add_signed(by)?;
        }
    }

    fn choose(&mut self, position: usize, cx: &mut Context<Self>) {
        let value = self.matches.get(position).map(|&index| &self.values[index]);
        if let Some(value) = value.filter(|value| !value.is_disabled) {
            cx.emit(ConfigPickerEvent::Chose(value.value.clone()));
        }
    }

    fn render_value(&self, position: usize, value: &LocalConfigValue, cx: &mut Context<Self>) -> ListItem {
        let theme = DesktopTheme::global(cx).ui();
        let colors = theme.colors;
        let current = value.value == self.current;
        ListItem::new(format!("{}-{}", self.selector, value.value), theme)
            .selected(position == self.highlighted)
            .when(!value.is_disabled, |row| {
                row.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(position, cx)))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(value.is_disabled, |name| name.text_color(colors.text_muted))
                    .child(value.name.clone()),
            )
            .when(value.is_disabled, |row| {
                row.child(div().typography(Typography::Caption).text_color(colors.text_muted).child("unavailable"))
            })
            .when(current, |row| row.child(Icon::new(IconName::Check, colors.text_muted)))
    }
}

impl EventEmitter<ConfigPickerEvent> for ConfigPicker {}

impl Render for ConfigPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.needs_focus) {
            let focus = self.focus_handle.clone();
            let input = self.search.as_ref().map(|(input, _)| input.clone());
            window.defer(cx, move |window, cx| match input {
                Some(input) => input.update(cx, |input, cx| input.focus(window, cx)),
                None => focus.focus(window, cx),
            });
        }
        let theme = DesktopTheme::global(cx).ui();
        let colors = theme.colors;
        let rows: Vec<ListItem> = self
            .matches
            .clone()
            .into_iter()
            .enumerate()
            .map(|(position, index)| self.render_value(position, &self.values[index].clone(), cx))
            .collect();
        let search = self.search.as_ref().map(|(input, _)| {
            let selector = format!("{}-search", self.selector);
            div()
                .debug_selector(move || selector)
                .flex_none()
                .h(rem(BAR_HEIGHT))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(colors.border)
                .child(Icon::new(IconName::Search, colors.text_muted))
                .child(div().flex_1().min_w_0().child(Input::new(input)))
        });
        let list = if rows.is_empty() {
            div().p_3().typography(Typography::Body).text_color(colors.text_muted).child("No matches")
        } else {
            div().flex_1().min_h_0().child(
                div()
                    .id(gpui::SharedString::from(format!("{}-list", self.selector)))
                    .max_h(rem(LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .p_1()
                    .children(rows),
            )
        };
        let selector = format!("{}-picker", self.selector);
        div()
            .debug_selector(move || selector)
            .track_focus(&self.focus_handle)
            .key_context(CONTEXT)
            .w(rem(PICKER_WIDTH))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(rem(RADIUS_CONTAINER))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .shadow_lg()
            // Captured on the way down to the search field, which would otherwise move its cursor.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.choose(this.highlighted, cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(ConfigPickerEvent::Dismissed)))
            .child(
                div()
                    .flex_none()
                    .px_3()
                    .pt_2()
                    .pb_1()
                    .typography(Typography::Label)
                    .text_color(colors.text_muted)
                    .child(self.name.clone()),
            )
            .children(search)
            .child(list)
    }
}

const PICKER_WIDTH: f32 = 288.0;
/// Tall enough for ten rows, short enough to fit above the composer in the smallest window.
const LIST_MAX_HEIGHT: f32 = 288.0;
