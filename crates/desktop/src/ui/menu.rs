use super::icon::{Icon, IconName};
use super::list_item::ListItem;
use super::tokens::{MENU_WIDTH, RADIUS_CONTAINER};
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{Anchor, App, IntoElement, MouseButton, SharedString, Window, div, prelude::*, px};
use gpui_base::{Popover, Selectable};

/// One entry in a menu, tagged with its selector.
pub struct MenuItem {
    selector: SharedString,
    label: SharedString,
    icon: Option<IconName>,
    action: MenuAction,
}

impl MenuItem {
    pub fn new(
        selector: impl Into<SharedString>,
        label: impl Into<SharedString>,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self { selector: selector.into(), label: label.into(), icon: None, action: Box::new(action) }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }
}

/// Wraps `trigger`, tagged `selector`, so that clicking it with `button` opens `items` in a
/// menu below it, tagged `<selector>-menu`. Choosing an entry closes the menu, then runs the
/// entry's action.
pub fn with_menu<T: Selectable + IntoElement + 'static>(
    selector: &str,
    trigger: T,
    button: MouseButton,
    items: Vec<MenuItem>,
    theme: &UiTheme,
) -> Popover {
    let theme = *theme;
    let menu_selector = format!("{selector}-menu");
    Popover::new(SharedString::from(format!("{selector}-popover")))
        .mouse_button(button)
        .anchor(Anchor::TopLeft)
        .trigger(trigger)
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let colors = theme.colors;
            let entries = items.into_iter().map(move |MenuItem { selector, label, icon, action }| {
                let popover = popover.clone();
                ListItem::new(selector, theme)
                    .on_click(move |_, window, cx| {
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        action(window, cx);
                    })
                    .children(icon.map(|icon| Icon::new(icon, colors.text_muted)))
                    .child(div().min_w_0().truncate().child(label))
            });
            div()
                .debug_selector(move || menu_selector)
                .w(px(MENU_WIDTH))
                .p_1()
                .flex()
                .flex_col()
                .rounded(px(RADIUS_CONTAINER))
                .border_1()
                .border_color(colors.border)
                .bg(colors.surface)
                .shadow_lg()
                .children(entries)
        })
}

type MenuAction = Box<dyn Fn(&mut Window, &mut App) + 'static>;
