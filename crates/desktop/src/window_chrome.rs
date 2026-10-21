use crate::menus::APP_NAME;
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{
    AnyElement, App, BoxShadow, CursorStyle, Decorations, FontWeight, MouseButton, Pixels, Point, ResizeEdge, Size,
    Tiling, Window, WindowButton, WindowButtonLayout, WindowControlArea, WindowControls, div, hsla, point, prelude::*,
    px, transparent_black,
};

const CLIENT_INSET: Pixels = px(8.0);
const BORDER_SIZE: Pixels = px(1.0);
const TITLE_BAR_HEIGHT: Pixels = px(36.0);
const BUTTON_WIDTH: Pixels = px(42.0);

/// Wraps `content` in a title bar, frame and resize zones when the platform
/// asks the app to draw its own decorations (Wayland compositors without
/// server-side decorations, and X11).
pub(crate) fn decorate(content: AnyElement, theme: &UiTheme, window: &mut Window, cx: &mut App) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        window.set_client_inset(px(0.0));
        return content;
    };

    window.set_client_inset(CLIENT_INSET);
    let window_size = window.window_bounds().get_bounds().size;
    let is_resizable = window.is_resizable();
    let is_minimizable = window.is_minimizable();
    let controls = window.window_controls();
    let layout = cx.button_layout().unwrap_or(WindowButtonLayout {
        left: [None, None, None],
        right: [Some(WindowButton::Minimize), Some(WindowButton::Maximize), Some(WindowButton::Close)],
    });
    let left_controls = supported_buttons(layout.left, controls, is_resizable, is_minimizable);
    let right_controls = supported_buttons(layout.right, controls, is_resizable, is_minimizable);

    let framed_content = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h_0()
        .min_w_0()
        .overflow_hidden()
        .border_color(theme.colors.border)
        .when(!tiling.top, |element| element.border_t(BORDER_SIZE))
        .when(!tiling.bottom, |element| element.border_b(BORDER_SIZE))
        .when(!tiling.left, |element| element.border_l(BORDER_SIZE))
        .when(!tiling.right, |element| element.border_r(BORDER_SIZE))
        .when(!tiling.is_tiled(), |element| {
            element.shadow(vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, 0.35),
                blur_radius: px(6.0),
                spread_radius: px(-1.0),
                offset: point(px(0.0), px(1.0)),
                inset: false,
            }])
        })
        .child(title_bar(
            theme,
            left_controls,
            right_controls,
            controls.maximize && is_resizable,
            window.is_maximized(),
        ))
        .child(div().flex_1().min_h_0().min_w_0().child(content));

    div()
        .id("window-backdrop")
        .size_full()
        .flex()
        .flex_col()
        .bg(transparent_black())
        .when(!tiling.top, |element| element.pt(CLIENT_INSET))
        .when(!tiling.bottom, |element| element.pb(CLIENT_INSET))
        .when(!tiling.left, |element| element.pl(CLIENT_INSET))
        .when(!tiling.right, |element| element.pr(CLIENT_INSET))
        .when(is_resizable, |element| {
            element.on_mouse_down(MouseButton::Left, move |event, window, cx| {
                if let Some(edge) = resize_edge(event.position, window_size, CLIENT_INSET, tiling) {
                    cx.stop_propagation();
                    window.start_window_resize(edge);
                }
            })
        })
        .child(framed_content)
        .children(resize_hit_zones(window_size, CLIENT_INSET, tiling, is_resizable))
        .into_any_element()
}

fn title_bar(
    theme: &UiTheme,
    left_controls: Vec<WindowButton>,
    right_controls: Vec<WindowButton>,
    can_maximize: bool,
    is_maximized: bool,
) -> impl IntoElement {
    div()
        .id("title-bar")
        .window_control_area(WindowControlArea::Drag)
        .h(TITLE_BAR_HEIGHT)
        .w_full()
        .flex()
        .items_center()
        .bg(theme.colors.surface)
        .text_color(theme.colors.text)
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            if can_maximize && event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .child(window_button_group(theme, left_controls, is_maximized))
        .child(div().flex_1().min_w_0().text_center().text_sm().font_weight(FontWeight::MEDIUM).child(APP_NAME))
        .child(window_button_group(theme, right_controls, is_maximized))
}

fn window_button_group(theme: &UiTheme, buttons: Vec<WindowButton>, is_maximized: bool) -> impl IntoElement {
    div().h_full().flex().children(buttons.into_iter().map(|button| {
        let (glyph, area) = match button {
            WindowButton::Minimize => ("—", WindowControlArea::Min),
            WindowButton::Maximize if is_maximized => ("❐", WindowControlArea::Max),
            WindowButton::Maximize => ("□", WindowControlArea::Max),
            WindowButton::Close => ("×", WindowControlArea::Close),
        };
        let hover = if button == WindowButton::Close { theme.colors.destructive } else { theme.colors.surface_hover };
        div()
                .id(button.id())
                .window_control_area(area)
                .h_full()
                .w(BUTTON_WIDTH)
                .flex()
                .items_center()
                .justify_center()
                .text_sm()
                .cursor(CursorStyle::PointingHand)
                .hover(move |element| element.bg(hover))
                // Keeps the title bar's mouse-down handler from starting a window move.
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    match button {
                        WindowButton::Minimize => window.minimize_window(),
                        WindowButton::Maximize => window.zoom_window(),
                        WindowButton::Close => window.remove_window(),
                    }
                })
                .child(glyph)
    }))
}

fn supported_buttons(
    buttons: [Option<WindowButton>; 3],
    controls: WindowControls,
    is_resizable: bool,
    is_minimizable: bool,
) -> Vec<WindowButton> {
    buttons
        .into_iter()
        .flatten()
        .filter(|button| match button {
            WindowButton::Minimize => controls.minimize && is_minimizable,
            WindowButton::Maximize => controls.maximize && is_resizable,
            WindowButton::Close => true,
        })
        .collect()
}

fn resize_hit_zones(size: Size<Pixels>, inset: Pixels, tiling: Tiling, is_resizable: bool) -> Vec<AnyElement> {
    if !is_resizable {
        return Vec::new();
    }

    let right = size.width - inset;
    let bottom = size.height - inset;
    let zero = px(0.0);
    [
        (!tiling.top, ResizeEdge::Top, zero, zero, size.width, inset),
        (!tiling.bottom, ResizeEdge::Bottom, zero, bottom, size.width, inset),
        (!tiling.left, ResizeEdge::Left, zero, zero, inset, size.height),
        (!tiling.right, ResizeEdge::Right, right, zero, inset, size.height),
        (!tiling.top && !tiling.left, ResizeEdge::TopLeft, zero, zero, inset, inset),
        (!tiling.top && !tiling.right, ResizeEdge::TopRight, right, zero, inset, inset),
        (!tiling.bottom && !tiling.left, ResizeEdge::BottomLeft, zero, bottom, inset, inset),
        (!tiling.bottom && !tiling.right, ResizeEdge::BottomRight, right, bottom, inset, inset),
    ]
    .into_iter()
    .filter(|(enabled, ..)| *enabled)
    .map(|(_, edge, left, top, width, height)| {
        div().absolute().left(left).top(top).w(width).h(height).cursor(cursor_for_edge(edge)).into_any_element()
    })
    .collect()
}

fn resize_edge(position: Point<Pixels>, size: Size<Pixels>, inset: Pixels, tiling: Tiling) -> Option<ResizeEdge> {
    let on_top = !tiling.top && position.y < inset;
    let on_bottom = !tiling.bottom && position.y >= size.height - inset;
    let on_left = !tiling.left && position.x < inset;
    let on_right = !tiling.right && position.x >= size.width - inset;

    match (on_top, on_bottom, on_left, on_right) {
        (true, _, true, _) => Some(ResizeEdge::TopLeft),
        (true, _, _, true) => Some(ResizeEdge::TopRight),
        (_, true, true, _) => Some(ResizeEdge::BottomLeft),
        (_, true, _, true) => Some(ResizeEdge::BottomRight),
        (true, _, _, _) => Some(ResizeEdge::Top),
        (_, true, _, _) => Some(ResizeEdge::Bottom),
        (_, _, true, _) => Some(ResizeEdge::Left),
        (_, _, _, true) => Some(ResizeEdge::Right),
        _ => None,
    }
}

fn cursor_for_edge(edge: ResizeEdge) -> CursorStyle {
    match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    }
}
