use aether_desktop::AppearanceMode;
use aether_desktop::registry::STATE_FILE;
use aether_desktop::testing::{DesktopHarness, DesktopHarnessBuilder};
use rouge::core::Point;
use std::fs;

#[test]
fn the_shortcut_opens_settings_and_escape_closes_it() {
    let mut harness = DesktopHarnessBuilder::default().build();

    harness.press("secondary-,");
    assert!(harness.bounds("settings").is_some());

    harness.press("escape");
    assert!(harness.bounds("settings").is_none());
}

#[test]
fn clicking_outside_settings_closes_it() {
    let mut harness = DesktopHarnessBuilder::default().build();

    harness.click("open-settings").unwrap();
    harness.click("settings").unwrap();
    assert!(harness.bounds("settings").is_some(), "a click inside keeps it open");

    click_scrim(&mut harness, "settings-backdrop");
    assert!(harness.bounds("settings").is_none());
}

#[test]
fn choosing_an_appearance_shows_and_saves_it() {
    let mut harness = DesktopHarnessBuilder::default().build();
    harness.press("secondary-,");

    harness.click("appearance-dark").unwrap();
    assert_eq!(theme_id(&mut harness), "github-dark");
    assert_eq!(saved_mode(&harness), AppearanceMode::Dark);

    harness.click("appearance-system").unwrap();
    assert_eq!(theme_id(&mut harness), "github-light", "the test window looks light");
}

#[test]
fn the_light_theme_picker_previews_the_highlighted_theme_and_saves_the_chosen_one() {
    let mut harness = DesktopHarnessBuilder::default().build();
    harness.press("secondary-,");
    harness.click("light-theme").unwrap();
    assert!(harness.bounds("github-dark").is_none(), "only light themes are offered");

    harness.type_text("solarized");
    assert_eq!(theme_id(&mut harness), "solarized-light", "the whole app previews the highlighted theme");
    assert_eq!(saved_pair(&harness), ("github-light".into(), "github-dark".into()));

    harness.press("enter");
    assert_eq!(saved_pair(&harness), ("solarized-light".into(), "github-dark".into()));
    assert_eq!(theme_id(&mut harness), "solarized-light");
    assert!(harness.bounds("light-themes").is_none());
    assert!(harness.bounds("settings").is_some(), "choosing a theme keeps Settings open");
}

#[test]
fn clicking_a_theme_in_the_picker_saves_it_without_closing_settings() {
    let mut harness = DesktopHarnessBuilder::default().build();
    harness.press("secondary-,");
    harness.click("light-theme").unwrap();
    harness.type_text("alaba");

    harness.click("alabaster").unwrap();

    assert_eq!(saved_pair(&harness).0, "alabaster");
    assert_eq!(theme_id(&mut harness), "alabaster");
    assert!(harness.bounds("settings").is_some());
}

#[test]
fn escape_cancels_the_theme_picker_and_then_closes_settings() {
    let mut harness = DesktopHarnessBuilder::default().build();
    harness.press("secondary-,");
    harness.click("light-theme").unwrap();

    harness.press("down");
    assert_ne!(theme_id(&mut harness), "github-light");

    harness.press("escape");
    assert_eq!(theme_id(&mut harness), "github-light", "cancelling shows the saved theme again");
    assert!(harness.bounds("light-themes").is_none());
    assert!(harness.bounds("settings").is_some());

    harness.press("escape");
    assert!(harness.bounds("settings").is_none());
}

#[test]
fn closing_settings_mid_preview_shows_the_saved_theme() {
    let mut harness = DesktopHarnessBuilder::default().build();
    harness.press("secondary-,");
    harness.click("dark-theme").unwrap();
    harness.type_text("nord");
    assert_eq!(theme_id(&mut harness), "nord");

    click_scrim(&mut harness, "settings-backdrop");
    assert!(harness.bounds("dark-themes").is_none(), "the first click outside closes the picker");
    assert_eq!(theme_id(&mut harness), "github-light");
    click_scrim(&mut harness, "settings-backdrop");

    assert!(harness.bounds("settings").is_none());
    assert_eq!(theme_id(&mut harness), "github-light");
    assert_eq!(saved_pair(&harness).1, "github-dark");
}

#[test]
fn saving_the_dark_theme_while_the_app_looks_light_keeps_it_light() {
    let mut harness = DesktopHarnessBuilder::default().build();
    harness.press("secondary-,");
    harness.click("dark-theme").unwrap();

    harness.type_text("nord");
    harness.press("enter");

    assert_eq!(saved_pair(&harness).1, "nord");
    assert_eq!(theme_id(&mut harness), "github-light");
}

#[test]
fn the_app_starts_with_the_saved_theme() {
    let dir = tempfile::tempdir().unwrap();
    let state = r#"{ "theme": { "mode": "dark", "pair": { "dark": "nord" } } }"#;
    fs::write(dir.path().join(STATE_FILE), state).unwrap();

    let mut harness = DesktopHarnessBuilder::default().state_dir(dir.path()).build();

    assert_eq!(theme_id(&mut harness), "nord");
}

#[test]
fn reduced_motion_is_live_and_remembered_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (agent, mut requests) = acp_utils::testing::FakeAgent::new("alpha-session").capture();
    let (mut harness, thread) =
        DesktopHarnessBuilder::default().state_dir(dir.path()).agent("/alpha", agent).start("/alpha");
    let _turn = harness.start_turn(thread, &mut requests, "work");
    harness.press("secondary-,");
    harness.click("motion-reduced").unwrap();
    harness.press("escape");
    harness.ui().run(2.0);

    assert!(harness.registry().with(|state| state.reduced_motion));
    assert!(!harness.ui().is_animating());
    assert!(harness.bounds("activity").is_some(), "reduced motion retains the activity indication");
    harness.quit();

    let mut harness = DesktopHarnessBuilder::default().state_dir(dir.path()).build();
    assert!(harness.registry().with(|state| state.reduced_motion));
    harness.press("secondary-,");
    harness.click("motion-full").unwrap();
    assert!(!harness.registry().with(|state| state.reduced_motion));
}

fn theme_id(harness: &mut DesktopHarness) -> String {
    harness.theme().id().to_string()
}

fn saved_mode(harness: &DesktopHarness) -> AppearanceMode {
    harness.registry().with(|state| state.theme.mode)
}

fn saved_pair(harness: &DesktopHarness) -> (String, String) {
    harness.registry().with(|state| (state.theme.pair.light.clone(), state.theme.pair.dark.clone()))
}

fn click_scrim(harness: &mut DesktopHarness, backdrop: &str) {
    let bounds = harness.bounds(backdrop).expect("the scrim is painted");
    harness.click_at(Point::new(bounds.origin.x + 4.0, bounds.origin.y + 4.0));
}
