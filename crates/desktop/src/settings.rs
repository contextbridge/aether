use crate::registry::Registry;
use crate::theme::{self, DesktopTheme};
use crate::ui::{
    Button, ButtonVariant, ControlSize, Modal, ModalSize, ThemePicker, ThemePickerEvent, Typography, TypographyExt,
    UiTheme,
};
use clankerdiff_ui_core::{Appearance, AppearanceMode, ReviewTheme, ThemeChoice, ThemePreference};
use gpui::{
    Anchor, AnyElement, App, Context, Entity, EventEmitter, FocusHandle, KeyBinding, Subscription, Window, actions,
    div, prelude::*,
};
use gpui_base::Popover;

actions!(settings, [CloseSettings,]);

pub(crate) struct Settings {
    registry: Entity<Registry>,
    picker: Option<(Appearance, Entity<ThemePicker>, Subscription)>,
    focus_handle: FocusHandle,
}

pub(crate) struct Closed;

impl Settings {
    pub(crate) fn bind_keys(cx: &mut App) {
        cx.bind_keys([KeyBinding::new("escape", CloseSettings, Some(CONTEXT))]);
    }

    pub(crate) fn new(registry: Entity<Registry>, cx: &mut Context<Self>) -> Self {
        Self { registry, picker: None, focus_handle: cx.focus_handle() }
    }

    pub(crate) fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus_handle.focus(window, cx);
    }

    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.picker.take().is_some() {
            self.show_saved_theme(window, cx);
        }
        cx.emit(Closed);
    }

    fn set_mode(&mut self, mode: AppearanceMode, window: &mut Window, cx: &mut Context<Self>) {
        Registry::update(&self.registry, cx, |state| state.theme.mode = mode);
        self.show_saved_theme(window, cx);
    }

    fn open_picker(&mut self, appearance: Appearance, window: &mut Window, cx: &mut Context<Self>) {
        let id = self.preference(cx).pair.id(appearance).to_owned();
        let current = ReviewTheme::builtin(&id).unwrap_or_default();
        let ui = DesktopTheme::global(cx).ui();
        let picker = cx.new(|cx| {
            let themes = ThemeChoice::catalog_for(appearance);
            ThemePicker::new(format!("{}-themes", key(appearance)), &current, themes, ui, window, cx)
        });
        let subscription = cx.subscribe_in(&picker, window, move |this, _, event: &ThemePickerEvent, window, cx| {
            this.on_picker_event(appearance, event, window, cx);
        });
        self.picker = Some((appearance, picker, subscription));
        cx.notify();
    }

    fn cancel_picker(&self, cx: &mut App) {
        if let Some((_, picker, _)) = &self.picker {
            picker.update(cx, ThemePicker::cancel);
        }
    }

    fn on_picker_event(
        &mut self,
        appearance: Appearance,
        event: &ThemePickerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ThemePickerEvent::Preview(theme) => theme::preview(theme.clone(), window, cx),
            ThemePickerEvent::Commit(theme) => {
                let id = theme.id().to_string();
                Registry::update(&self.registry, cx, |state| state.theme.pair.set(appearance, id));
                self.close_picker(window, cx);
            }
            ThemePickerEvent::Cancel(_) => self.close_picker(window, cx),
        }
    }

    fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.picker = None;
        self.show_saved_theme(window, cx);
        self.focus(window, cx);
        cx.notify();
    }

    fn show_saved_theme(&self, window: &mut Window, cx: &mut App) {
        let preference = self.preference(cx).clone();
        theme::apply(&preference, window, cx);
    }

    fn preference<'a>(&self, cx: &'a App) -> &'a ThemePreference {
        &self.registry.read(cx).state().theme
    }

    fn render_mode(&self, theme: &UiTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.preference(cx).mode;
        div().flex().gap_1().children(
            [(AppearanceMode::System, "System"), (AppearanceMode::Light, "Light"), (AppearanceMode::Dark, "Dark")].map(
                |(mode, label)| {
                    Button::new(format!("appearance-{}", label.to_lowercase()), label, *theme)
                        .size(ControlSize::Small)
                        .selected(mode == current)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_mode(mode, window, cx)))
                },
            ),
        )
    }

    fn render_theme(&self, appearance: Appearance, theme: &UiTheme, cx: &mut Context<Self>) -> impl IntoElement {
        let key = key(appearance);
        let name = theme_name(self.preference(cx).pair.id(appearance));
        let picker = self.picker.as_ref().filter(|(open, ..)| *open == appearance).map(|(_, picker, _)| picker.clone());
        let settings = cx.entity().downgrade();
        Popover::new(format!("{key}-theme-popover"))
            .anchor(Anchor::TopRight)
            .open(picker.is_some())
            .on_open_change(move |open, window, cx| {
                let _ = settings.update(cx, |this, cx| {
                    if *open {
                        this.open_picker(appearance, window, cx);
                    } else {
                        this.cancel_picker(cx);
                    }
                });
            })
            .trigger(Button::new(format!("{key}-theme"), name, *theme).variant(ButtonVariant::Secondary))
            .content(move |_, _, _| div().children(picker))
    }
}

impl EventEmitter<Closed> for Settings {}

impl Render for Settings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let settings = cx.entity().downgrade();
        let modal = Modal::new("settings", "Settings", theme)
            .hint("Esc to close")
            .size(ModalSize::Compact)
            .on_dismiss(move |window, cx| {
                let _ = settings.update(cx, |this, cx| this.close(window, cx));
            })
            .child(row(
                "Appearance",
                "Follow the system, or always look light or dark",
                self.render_mode(&theme, cx),
                &theme,
            ))
            .child(row(
                "Light theme",
                "Shown when the app looks light",
                self.render_theme(Appearance::Light, &theme, cx),
                &theme,
            ))
            .child(row(
                "Dark theme",
                "Shown when the app looks dark",
                self.render_theme(Appearance::Dark, &theme, cx),
                &theme,
            ));
        div()
            .absolute()
            .inset_0()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &CloseSettings, window, cx| this.close(window, cx)))
            .child(modal)
    }
}

const CONTEXT: &str = "Settings";

fn key(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => "light",
        Appearance::Dark => "dark",
    }
}

fn theme_name(id: &str) -> String {
    ReviewTheme::catalog().into_iter().find(|theme| theme.id == id).map_or_else(|| id.to_owned(), |theme| theme.name)
}

fn row(label: &'static str, detail: &'static str, control: impl IntoElement, theme: &UiTheme) -> AnyElement {
    div()
        .py_2()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .child(label)
                .child(div().typography(Typography::Small).text_color(theme.colors.text_muted).child(detail)),
        )
        .child(control)
        .into_any_element()
}
