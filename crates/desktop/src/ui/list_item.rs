use super::tokens::{RADIUS_CONTROL, ROW_HEIGHT};
use super::typography::{Typography, TypographyExt};
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{
    AnyElement, App, ClickEvent, ElementId, IntoElement, RenderOnce, SharedString, Window, div, prelude::*, px,
};
use gpui_base::Selectable;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A 28px row laying its children out in a line, filled while selected and on hover.
///
/// Its id doubles as its debug selector, and names the hover group `<id>-hover`, so children
/// can show themselves only while the pointer is over the row.
#[derive(IntoElement)]
pub struct ListItem {
    id: SharedString,
    selected: bool,
    theme: UiTheme,
    on_click: Option<ClickHandler>,
    children: Vec<AnyElement>,
}

impl ListItem {
    pub fn new(id: impl Into<SharedString>, theme: UiTheme) -> Self {
        Self { id: id.into(), selected: false, theme, on_click: None, children: Vec::new() }
    }

    /// The hover group of the row tagged `id`.
    pub fn hover_group(id: &str) -> SharedString {
        format!("{id}-hover").into()
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Selectable for ListItem {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl ParentElement for ListItem {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ListItem {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        let selector = self.id.to_string();
        div()
            .id(ElementId::Name(self.id.clone()))
            .group(Self::hover_group(&self.id))
            .debug_selector(move || selector)
            .w_full()
            .min_w_0()
            .h(px(ROW_HEIGHT))
            .px_2()
            .flex()
            .items_center()
            .gap_2()
            .rounded(px(RADIUS_CONTROL))
            .typography(Typography::Body)
            .text_color(colors.text)
            .when(self.selected, |row| row.bg(colors.surface_selected))
            .when(!self.selected, |row| row.hover(move |row| row.bg(colors.surface_hover)))
            .when_some(self.on_click, |row, handler| row.cursor_pointer().on_click(handler))
            .children(self.children)
    }
}
