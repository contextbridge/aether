use super::icon::{Icon, IconName};
use super::tokens::{ICON_BUTTON_SIZE, RADIUS_CONTAINER, RADIUS_CONTROL, ROW_HEIGHT};
use super::typography::{Typography, TypographyExt};
use clankerdiff_gpui::ui::prelude::{ControlState, InteractionState, UiTheme};
use gpui::{
    App, ClickEvent, Context, ElementId, IntoElement, Render, RenderOnce, Role, SharedString, Window, div, prelude::*,
    px,
};
use gpui_base::Selectable;

pub use clankerdiff_gpui::ui::prelude::ButtonVariant;

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// A labelled button, 28px tall, with an optional leading icon.
///
/// Its id doubles as its debug selector.
#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<IconName>,
    variant: ButtonVariant,
    selected: bool,
    disabled: bool,
    full_width: bool,
    theme: UiTheme,
    on_click: Option<ClickHandler>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>, theme: UiTheme) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            variant: ButtonVariant::Secondary,
            selected: false,
            disabled: false,
            full_width: false,
            theme,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Stretches the button across its container, with its label at the start.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Selectable for Button {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let interaction = if self.disabled { InteractionState::Disabled } else { InteractionState::Rest };
        let style = self.theme.control_style(self.variant, ControlState { interaction, selected: self.selected });
        let hover = self.theme.control_style(
            self.variant,
            ControlState { interaction: InteractionState::Hovered, selected: self.selected },
        );
        let selector = self.id.to_string();
        div()
            .id(self.id)
            .debug_selector(move || selector)
            .role(Role::Button)
            .flex_none()
            .h(px(ROW_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .when(self.full_width, Styled::w_full)
            .when(!self.full_width, Styled::justify_center)
            .rounded(px(RADIUS_CONTROL))
            .typography(Typography::BodyStrong)
            .text_color(style.foreground)
            .when_some(style.background, Styled::bg)
            .when(self.variant == ButtonVariant::Secondary, |button| {
                button.border_1().border_color(self.theme.colors.border)
            })
            .when(!self.disabled, |button| {
                button
                    .cursor_pointer()
                    .hover(move |button| {
                        let button = button.text_color(hover.foreground);
                        match hover.background {
                            Some(background) => button.bg(background),
                            None => button,
                        }
                    })
                    .when_some(self.on_click, |button, handler| button.on_click(handler))
            })
            .children(self.icon.map(|icon| Icon::new(icon, style.foreground)))
            .child(div().min_w_0().truncate().child(self.label))
    }
}

/// A square, borderless button holding one icon, labelled by a tooltip.
///
/// Muted at rest, it takes the text color on hover and while selected or open, as when it
/// triggers a menu. Its id doubles as its debug selector.
#[derive(IntoElement)]
pub struct IconButton {
    id: SharedString,
    icon: IconName,
    label: SharedString,
    selected: bool,
    theme: UiTheme,
    on_click: Option<ClickHandler>,
}

impl IconButton {
    pub fn new(id: impl Into<SharedString>, icon: IconName, label: impl Into<SharedString>, theme: UiTheme) -> Self {
        Self { id: id.into(), icon, label: label.into(), selected: false, theme, on_click: None }
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl Selectable for IconButton {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors;
        let selector = self.id.to_string();
        let label = self.label.clone();
        let theme = self.theme;
        let color = if self.selected { colors.text } else { colors.text_muted };
        let group = format!("{}-hover", self.id);
        div()
            .id(ElementId::Name(self.id))
            .group(group.clone())
            .debug_selector(move || selector)
            .role(Role::Button)
            .aria_label(self.label)
            .flex_none()
            .size(px(ICON_BUTTON_SIZE))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(RADIUS_CONTROL))
            .cursor_pointer()
            .when(self.selected, |button| button.bg(colors.surface_selected))
            .hover(move |button| button.bg(colors.surface_hover))
            .tooltip(move |_, cx| cx.new(|_| Tooltip { label: label.clone(), theme }).into())
            // A row or header the button sits in must not also act on the click.
            .when_some(self.on_click, |button, handler| {
                button.on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event, window, cx);
                })
            })
            .child(Icon::new(self.icon, color).group_hover(group, colors.text))
    }
}

struct Tooltip {
    label: SharedString,
    theme: UiTheme,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let colors = self.theme.colors;
        div()
            .px_2()
            .py_1()
            .rounded(px(RADIUS_CONTAINER))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .shadow_md()
            .typography(Typography::Small)
            .text_color(colors.text)
            .child(self.label.clone())
    }
}
