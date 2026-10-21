use aether_desktop::{DesktopTheme, ThemePair, testing::DesktopHarnessBuilder};
use clankerdiff_theme::{ReviewTheme, ThemeId};
use gpui::{TestAppContext, WindowAppearance};

#[test]
fn light_appearances_pick_the_light_theme() {
    for appearance in [WindowAppearance::Light, WindowAppearance::VibrantLight] {
        assert_eq!(ThemePair::DEFAULT.theme_id(appearance), "github-light");
    }
}

#[test]
fn dark_appearances_pick_the_dark_theme() {
    for appearance in [WindowAppearance::Dark, WindowAppearance::VibrantDark] {
        assert_eq!(ThemePair::DEFAULT.theme_id(appearance), "github-dark");
    }
}

#[test]
fn default_pair_names_a_light_and_a_dark_builtin() {
    let catalog = ReviewTheme::catalog();
    let is_dark = |id: &str| catalog.iter().find(|theme| theme.id == id).map(|theme| theme.is_dark);

    assert_eq!(is_dark(&ThemePair::DEFAULT.light), Some(false));
    assert_eq!(is_dark(&ThemePair::DEFAULT.dark), Some(true));
}

#[test]
fn theme_resolves_the_pair_member_for_the_appearance() {
    let pair = ThemePair { light: "solarized-light".into(), dark: "solarized-dark".into() };

    let theme = DesktopTheme::new(pair.clone(), WindowAppearance::Dark);

    assert_eq!(theme.pair(), &pair);
    assert_eq!(theme.review().id(), &builtin("solarized-dark"));
}

#[test]
fn unknown_theme_falls_back_to_the_default() {
    let pair = ThemePair { light: "no-such-theme".into(), dark: "github-dark".into() };

    let theme = DesktopTheme::new(pair, WindowAppearance::Light);

    assert_eq!(theme.review().id(), ReviewTheme::default().id());
}

#[gpui::test]
fn window_starts_with_the_theme_for_the_system_appearance(cx: &mut TestAppContext) {
    // gpui's test windows report a light appearance.
    let harness = DesktopHarnessBuilder::default().build(cx);

    assert_eq!(harness.theme(cx).review().id(), &builtin("github-light"));
}

fn builtin(id: &str) -> ThemeId {
    ThemeId::Builtin(id.to_owned())
}
