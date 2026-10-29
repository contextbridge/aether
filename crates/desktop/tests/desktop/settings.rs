use aether_desktop::registry::STATE_FILE;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use aether_desktop::{AppearanceMode, DesktopTheme};
use gpui::{TestAppContext, point, px};
use std::fs;

#[gpui::test]
fn the_shortcut_opens_settings_and_escape_closes_it(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    harness.simulate_keystrokes(cx, "secondary-,");
    assert!(harness.bounds(cx, "settings").is_some());

    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "settings").is_none());
}

#[gpui::test]
fn clicking_outside_settings_closes_it(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);

    harness.click(cx, "open-settings").unwrap();
    harness.click(cx, "settings").unwrap();
    assert!(harness.bounds(cx, "settings").is_some(), "a click inside keeps it open");

    click_scrim(&harness, cx, "settings-backdrop");
    assert!(harness.bounds(cx, "settings").is_none());
}

#[gpui::test]
fn choosing_an_appearance_shows_and_saves_it(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.simulate_keystrokes(cx, "secondary-,");

    harness.click(cx, "appearance-dark").unwrap();
    assert_eq!(theme_id(cx), "github-dark");
    assert_eq!(saved_mode(&harness, cx), AppearanceMode::Dark);

    harness.click(cx, "appearance-system").unwrap();
    assert_eq!(theme_id(cx), "github-light", "the test window looks light");
}

#[gpui::test]
fn the_light_theme_picker_previews_the_highlighted_theme_and_saves_the_chosen_one(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.simulate_keystrokes(cx, "secondary-,");
    harness.click(cx, "light-theme").unwrap();
    assert!(harness.bounds(cx, "light-themes-github-dark").is_none(), "only light themes are offered");

    harness.simulate_input(cx, "solarized");
    assert_eq!(theme_id(cx), "solarized-light", "the whole app previews the highlighted theme");
    assert_eq!(saved_pair(&harness, cx), ("github-light".into(), "github-dark".into()));

    harness.simulate_keystrokes(cx, "enter");
    assert_eq!(saved_pair(&harness, cx), ("solarized-light".into(), "github-dark".into()));
    assert_eq!(theme_id(cx), "solarized-light");
    assert!(harness.bounds(cx, "light-themes").is_none());
    assert!(harness.bounds(cx, "settings").is_some(), "choosing a theme keeps Settings open");
}

#[gpui::test]
fn clicking_a_theme_in_the_picker_saves_it_without_closing_settings(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.simulate_keystrokes(cx, "secondary-,");
    harness.click(cx, "light-theme").unwrap();

    harness.click(cx, "light-themes-alabaster").unwrap();

    assert_eq!(saved_pair(&harness, cx).0, "alabaster");
    assert_eq!(theme_id(cx), "alabaster");
    assert!(harness.bounds(cx, "settings").is_some());
}

#[gpui::test]
fn escape_cancels_the_theme_picker_and_then_closes_settings(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.simulate_keystrokes(cx, "secondary-,");
    harness.click(cx, "light-theme").unwrap();

    harness.simulate_keystrokes(cx, "down");
    assert_ne!(theme_id(cx), "github-light");

    harness.simulate_keystrokes(cx, "escape");
    assert_eq!(theme_id(cx), "github-light", "cancelling shows the saved theme again");
    assert!(harness.bounds(cx, "light-themes").is_none());
    assert!(harness.bounds(cx, "settings").is_some());

    harness.simulate_keystrokes(cx, "escape");
    assert!(harness.bounds(cx, "settings").is_none());
}

#[gpui::test]
fn closing_settings_mid_preview_shows_the_saved_theme(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.simulate_keystrokes(cx, "secondary-,");
    harness.click(cx, "dark-theme").unwrap();
    harness.simulate_input(cx, "nord");
    assert_eq!(theme_id(cx), "nord");

    click_scrim(&harness, cx, "settings-backdrop");

    assert!(harness.bounds(cx, "settings").is_none());
    assert_eq!(theme_id(cx), "github-light");
    assert_eq!(saved_pair(&harness, cx).1, "github-dark");
}

#[gpui::test]
fn saving_the_dark_theme_while_the_app_looks_light_keeps_it_light(cx: &mut TestAppContext) {
    let harness = DesktopHarnessBuilder::default().build(cx);
    harness.simulate_keystrokes(cx, "secondary-,");
    harness.click(cx, "dark-theme").unwrap();

    harness.simulate_input(cx, "nord");
    harness.simulate_keystrokes(cx, "enter");

    assert_eq!(saved_pair(&harness, cx).1, "nord");
    assert_eq!(theme_id(cx), "github-light");
}

#[gpui::test]
fn the_app_starts_with_the_saved_theme(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let state = r#"{ "theme": { "mode": "dark", "pair": { "dark": "nord" } } }"#;
    fs::write(dir.path().join(STATE_FILE), state).unwrap();

    DesktopHarnessBuilder::default().state_dir(dir.path()).build(cx);

    assert_eq!(theme_id(cx), "nord");
}

fn theme_id(cx: &TestAppContext) -> String {
    cx.read(|cx| DesktopTheme::global(cx).review().id().to_string())
}

fn saved_mode(harness: &DesktopHarness, cx: &TestAppContext) -> AppearanceMode {
    harness.registry(cx).read_with(cx, |registry, _| registry.state().theme.mode)
}

fn saved_pair(harness: &DesktopHarness, cx: &TestAppContext) -> (String, String) {
    harness.registry(cx).read_with(cx, |registry, _| {
        let pair = &registry.state().theme.pair;
        (pair.light.clone(), pair.dark.clone())
    })
}

fn click_scrim(harness: &DesktopHarness, cx: &mut TestAppContext, backdrop: &'static str) {
    let bounds = harness.bounds(cx, backdrop).expect("the scrim is painted");
    harness.click_at(cx, bounds.origin + point(px(4.0), px(4.0)));
}
