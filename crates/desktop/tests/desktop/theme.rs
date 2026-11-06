use aether_desktop::{AppearanceMode, ThemePreference};
use clankerdiff_ui_core::{Appearance, ReviewTheme, ThemePair};

#[test]
fn unknown_theme_falls_back_to_the_default() {
    let pair = ThemePair { light: "no-such-theme".into(), dark: "github-dark".into() };
    let preference = ThemePreference { mode: AppearanceMode::System, pair };

    assert_eq!(preference.theme(Appearance::Light).id(), ReviewTheme::default().id());
}

#[test]
fn a_fixed_mode_picks_its_theme_whatever_the_system_shows() {
    let preference = ThemePreference { mode: AppearanceMode::Dark, pair: ThemePair::default() };

    assert_eq!(preference.theme(Appearance::Light).id().to_string(), "github-dark");
    assert_eq!(ThemePreference::default().theme(Appearance::Light).id().to_string(), "github-light");
}
