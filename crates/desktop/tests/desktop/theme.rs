use aether_desktop::{DesktopTheme, ThemePair};
use clankerdiff_theme::ReviewTheme;
use gpui::WindowAppearance;

#[test]
fn unknown_theme_falls_back_to_the_default() {
    let pair = ThemePair { light: "no-such-theme".into(), dark: "github-dark".into() };

    let theme = DesktopTheme::new(pair, WindowAppearance::Light);

    assert_eq!(theme.review().id(), ReviewTheme::default().id());
}
