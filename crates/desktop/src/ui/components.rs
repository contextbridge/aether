use super::tokens::{BAR_HEIGHT, CONTENT_MAX_WIDTH, ICON_SIZE, RADIUS_CONTROL, rem};
use super::{SearchPickerEvent, Tooltip, Typography, TypographyExt, UiTheme, WindowDragExt};
use crate::theme::DesktopTheme;
#[cfg(target_os = "macos")]
use gpui::point;
use gpui::{
    AnyElement, App, AppContext, DefiniteLength, Div, Entity, InteractiveElement, MouseButton, Pixels, Role,
    SharedString, Styled, Window, div, prelude::*, px, rems,
};
use gpui_base::input::{Input, InputState};

pub const WINDOW_BUTTONS_INSET: Pixels = px(WINDOW_BUTTONS_LEFT + WINDOW_BUTTONS_WIDTH + 8.0);

pub(crate) trait StyledExt: Styled + InteractiveElement + Sized {
    fn content_width(self) -> Self {
        self.w_full().max_w(rem(CONTENT_MAX_WIDTH)).mx_auto()
    }

    fn muted(self, theme: &UiTheme) -> Self {
        self.typography(Typography::Small).text_color(theme.colors.text_muted)
    }

    fn top_bar(self, leftmost: bool, window: &Window) -> Self {
        let clears_buttons = leftmost && cfg!(target_os = "macos") && !window.is_fullscreen();
        let left: DefiniteLength = if clears_buttons { WINDOW_BUTTONS_INSET.into() } else { rems(1.0).into() };
        self.flex_none().h(rem(BAR_HEIGHT)).flex().items_center().pl(left).window_drag_area()
    }

    fn reveal_on_hover(self, group: impl Into<SharedString>) -> Self {
        self.opacity(0.0).group_hover(group, |revealed| revealed.opacity(1.0))
    }
}

impl<T: Styled + InteractiveElement> StyledExt for T {}

#[cfg(target_os = "macos")]
pub(crate) fn place_window_buttons(window: &mut Window) {
    let bar = rem(BAR_HEIGHT).to_pixels(window.rem_size());
    window.set_traffic_light_position(point(px(WINDOW_BUTTONS_LEFT), (bar - px(WINDOW_BUTTONS_HEIGHT)) / 2.0));
}

#[cfg(not(target_os = "macos"))]
pub(crate) const fn place_window_buttons(_window: &mut Window) {}

pub(crate) fn in_top_bar(child: impl IntoElement) -> Div {
    div().flex_none().flex().items_center().no_window_drag().child(child)
}

pub(crate) fn text_input(value: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> Entity<InputState> {
    let value = value.into();
    cx.new(|cx| {
        cx.observe_global::<DesktopTheme>(|input: &mut InputState, cx| {
            input.set_editor_style(DesktopTheme::global(cx).ui().editor_style());
            cx.notify();
        })
        .detach();
        let mut input = InputState::new(window, cx).default_value(value);
        input.set_editor_style(DesktopTheme::global(cx).ui().editor_style());
        input
    })
}

pub(crate) fn text_field(input: &Entity<InputState>, theme: &UiTheme) -> Div {
    let focus = input.clone();
    div()
        .min_w_0()
        .px_2()
        .py_1()
        .rounded(rem(RADIUS_CONTROL))
        .border_1()
        .border_color(theme.colors.border)
        .bg(theme.colors.canvas)
        .cursor_text()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            focus.update(cx, |input, cx| input.focus(window, cx));
            cx.stop_propagation();
        })
        .child(Input::new(input))
}

pub(crate) fn icon_slot() -> Div {
    div().flex_none().size(rem(ICON_SIZE)).flex().items_center().justify_center()
}

pub(crate) fn text_selector(selector: &str, text: &str) -> String {
    format!("{selector}: {text}")
}

pub(crate) fn tagged(selector: String, child: impl IntoElement) -> Div {
    div().flex().debug_selector(move || selector).child(child)
}

pub(crate) fn with_middots(parts: impl IntoIterator<Item = AnyElement>, theme: &UiTheme) -> Vec<AnyElement> {
    let mut separated = Vec::new();
    for part in parts {
        if !separated.is_empty() {
            separated.push(div().flex_none().text_color(theme.colors.text_muted).child("·").into_any_element());
        }
        separated.push(part);
    }
    separated
}

pub(crate) fn hint(selector: String, label: &'static str, child: impl IntoElement, theme: &UiTheme) -> AnyElement {
    div()
        .id(selector.clone())
        .debug_selector(move || selector)
        .role(Role::Image)
        .aria_label(label)
        .flex_none()
        .flex()
        .items_center()
        .tooltip(Tooltip::text(label, *theme))
        .child(child)
        .into_any_element()
}

/// Hands a chosen index to `on_chose` and says whether the picker closed.
pub(crate) fn picker_closed(event: &SearchPickerEvent, on_chose: impl FnOnce(usize)) -> bool {
    match event {
        SearchPickerEvent::Chose(index) => {
            on_chose(*index);
            true
        }
        SearchPickerEvent::Dismissed => true,
        SearchPickerEvent::Query(_) | SearchPickerEvent::Highlighted(_) => false,
    }
}

const WINDOW_BUTTONS_LEFT: f32 = 16.0;
const WINDOW_BUTTONS_WIDTH: f32 = 54.0;
#[cfg(target_os = "macos")]
const WINDOW_BUTTONS_HEIGHT: f32 = 14.0;
