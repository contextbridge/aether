use super::thread_row::ThreadRow;
use clankerdiff_gpui::ui::prelude::{ListRow, Surface, UiTheme};
use gpui::{Anchor, App, MouseButton, Window, div, prelude::*, px};
use gpui_base::Popover;

/// One entry in a thread's context menu, tagged with its selector.
pub(crate) struct MenuItem {
    selector: &'static str,
    label: &'static str,
    action: MenuAction,
}

impl MenuItem {
    pub(crate) fn new(
        selector: &'static str,
        label: &'static str,
        action: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self { selector, label, action: Box::new(action) }
    }
}

/// Wraps `row`, tagged `selector`, so that right-clicking it opens `items` in a menu below it,
/// tagged `<selector>-menu`. Choosing an entry closes the menu, then runs the entry's action.
pub(crate) fn with_menu(selector: &str, row: ThreadRow, items: Vec<MenuItem>, theme: &UiTheme) -> Popover {
    let theme = *theme;
    let menu_selector = format!("{selector}-menu");
    Popover::new(format!("{selector}-popover"))
        .w_full()
        .mouse_button(MouseButton::Right)
        .anchor(Anchor::TopLeft)
        .trigger(row)
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let entries = items.into_iter().map(move |MenuItem { selector, label, action }| {
                let popover = popover.clone();
                let entry = ListRow::new(selector, theme)
                    .on_click(move |_, window, cx| {
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        action(window, cx);
                    })
                    .child(label);
                div().debug_selector(move || selector.to_owned()).w_full().child(entry)
            });
            div()
                .debug_selector(move || menu_selector)
                .w(px(MENU_WIDTH))
                .child(Surface::new(theme).padded(false).raised(true).child(div().py_1().children(entries)))
        })
}

type MenuAction = Box<dyn Fn(&mut Window, &mut App) + 'static>;

const MENU_WIDTH: f32 = 180.0;
