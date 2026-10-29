use aether_desktop::{AppearanceMode, DesktopTheme, ThemePreference};
use clankerdiff_ui_core::{Appearance, ReviewTheme, ThemePair};

#[test]
fn unknown_theme_falls_back_to_the_default() {
    let pair = ThemePair { light: "no-such-theme".into(), dark: "github-dark".into() };
    let preference = ThemePreference { mode: AppearanceMode::System, pair };

    let theme = DesktopTheme::resolve(&preference, Appearance::Light);

    assert_eq!(theme.review().id(), ReviewTheme::default().id());
}

#[test]
fn a_fixed_mode_picks_its_theme_whatever_the_system_shows() {
    let preference = ThemePreference { mode: AppearanceMode::Dark, pair: ThemePair::default() };

    assert_eq!(DesktopTheme::resolve(&preference, Appearance::Light).review().id().to_string(), "github-dark");
    assert_eq!(
        DesktopTheme::resolve(&ThemePreference::default(), Appearance::Light).review().id().to_string(),
        "github-light"
    );
}
