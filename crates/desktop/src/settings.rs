use crate::registry::Registry;
use crate::theme;
use crate::ui::{
    ButtonVariant, ControlSize, GlassExt, ModalSize, ThemePickerEvent, UiStyles, backdrop, bare, button, modal_width,
    theme_picker, theme_picker_view,
};
use clankerdiff_rouge_shell::rouge::core::{AnyView, Div, Styled, View, div, dynamic, pct, text, view};
use clankerdiff_rouge_shell::rouge::reactive::{Callback, Signal, effect, memo, signal, untrack};
use clankerdiff_rouge_shell::rouge::widgets::{Placement, popover};
use clankerdiff_ui_core::{Appearance, AppearanceMode, ReviewTheme, ThemeChoice};

pub(crate) fn settings(registry: Registry, on_close: impl Fn() + 'static) -> impl View {
    let on_close = Callback::from(on_close);
    let close = move || {
        show_saved_theme(registry);
        on_close.call();
    };
    view! {
        <backdrop id="settings-backdrop" on_dismiss=close>
            <div test_id="settings" w={modal_width(ModalSize::Compact)} max_w=pct(100.0) p=16.0 flex col gap=8.0
                 overflow_hidden glass_sheet>
                <text heading primary>"Settings"</text>
                <text small muted>"Esc to close"</text>
                <div flex col min_h=0.0>
                    {row("Activity motion", "Still indicators keep every activity label", motion(registry))}
                    {row("Appearance", "Follow the system, or always look light or dark", mode(registry))}
                    {row("Light theme", "Shown when the app looks light", theme_choice(registry, Appearance::Light))}
                    {row("Dark theme", "Shown when the app looks dark", theme_choice(registry, Appearance::Dark))}
                </div>
            </div>
        </backdrop>
    }
}

fn motion(registry: Registry) -> AnyView {
    let reduced = memo(move || registry.with(|state| state.reduced_motion));
    dynamic(move || {
        let reduced = reduced.get();
        let buttons = [(false, "Full"), (true, "Reduced")].map(|(value, label)| {
            view! {
                <button id={format!("motion-{}", label.to_lowercase())} label size=ControlSize::Small
                        selected={value == reduced} keyboard_focusable
                        on_click={move || registry.update(|state| state.reduced_motion = value)}/>
            }
        });
        view! { <div flex gap=4.0>{..buttons}</div> }
    })
    .into_any()
}

fn mode(registry: Registry) -> AnyView {
    let current = memo(move || registry.with(|state| state.theme.mode));
    dynamic(move || {
        let current = current.get();
        let buttons =
            [(AppearanceMode::System, "System"), (AppearanceMode::Light, "Light"), (AppearanceMode::Dark, "Dark")].map(
                |(mode, label)| {
                    view! {
                        <button id={format!("appearance-{}", label.to_lowercase())} label size=ControlSize::Small
                                selected={mode == current} keyboard_focusable
                                on_click={move || {
                                    registry.update(|state| state.theme.mode = mode);
                                    show_saved_theme(registry);
                                }}/>
                    }
                },
            );
        view! { <div flex gap=4.0>{..buttons}</div> }
    })
    .into_any()
}

fn theme_choice(registry: Registry, appearance: Appearance) -> AnyView {
    let key = key(appearance);
    let open = signal(false);
    effect(move || {
        if !open.get() {
            untrack(|| show_saved_theme(registry));
        }
    });
    let name = memo(move || registry.with(|state| theme_name(state.theme.pair.id(appearance))));
    let trigger = dynamic(move || {
        let label = name.get();
        view! {
            <button id={format!("{key}-theme")} label variant=ButtonVariant::Secondary keyboard_focusable
                    on_click={move || open.set(true)}/>
        }
    });
    popover(open, trigger, move || picker(registry, appearance, open)).placement(Placement::Below).into_any()
}

fn picker(registry: Registry, appearance: Appearance, open: Signal<bool>) -> impl View {
    let id = registry.with_untracked(|state| state.theme.pair.id(appearance).to_owned());
    let current = ReviewTheme::builtin(&id).unwrap_or_default();
    let handle = theme_picker(&current, ThemeChoice::catalog_for(appearance), move |event| match event {
        ThemePickerEvent::Preview(theme) => theme::preview(theme),
        ThemePickerEvent::Commit(theme) => {
            let id = theme.id().to_string();
            registry.update(|state| state.theme.pair.set(appearance, id));
            open.set(false);
        }
        ThemePickerEvent::Cancel(_) => open.set(false),
    });
    bare(|| view! { <theme_picker_view id={format!("{}-themes", key(appearance))} handle/> })
}

fn show_saved_theme(registry: Registry) {
    let preference = registry.with_untracked(|state| state.theme.clone());
    theme::apply(&preference);
}

fn key(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => "light",
        Appearance::Dark => "dark",
    }
}

fn theme_name(id: &str) -> String {
    ReviewTheme::catalog().into_iter().find(|theme| theme.id == id).map_or_else(|| id.to_owned(), |theme| theme.name)
}

fn row(label: &'static str, detail: &'static str, control: AnyView) -> Div {
    view! {
        <div py=8.0 flex items_center justify_between gap=16.0>
            <div min_w=0.0 flex col>
                <text body primary>{label}</text>
                <text small muted>{detail}</text>
            </div>
            {control}
        </div>
    }
}
