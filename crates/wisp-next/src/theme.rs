use crate::settings::{UiSettings, resolve_theme_file_path};
use ratatui::style::Color;
use std::path::Path;
use std::sync::Arc;
use syntect::highlighting::{Highlighter, Theme as SyntectTheme, ThemeSet};
use syntect::parsing::Scope;
use tracing::warn;
use two_face::theme::EmbeddedThemeName;

#[derive(Clone, Debug)]
pub struct Theme {
    pub text_primary: Color,
    pub text_secondary: Color,
    pub background: Color,
    pub sidebar_bg: Color,
    pub accent: Color,
    pub heading: Color,
    pub link: Color,
    pub blockquote: Color,
    pub code_fg: Color,
    pub code_bg: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub info: Color,
    pub muted: Color,
    pub diff_added_fg: Color,
    pub diff_added_bg: Color,
    pub diff_removed_fg: Color,
    pub diff_removed_bg: Color,
    syntect: Arc<SyntectTheme>,
}

impl Theme {
    pub fn load(settings: &UiSettings) -> Self {
        resolve_theme_file_path(settings).map_or_else(Self::default, |path| Self::load_from_path(&path))
    }

    pub fn load_from_path(path: &Path) -> Self {
        match ThemeSet::get_theme(path) {
            Ok(theme) => Self::from_syntect(theme),
            Err(error) => {
                warn!("Failed to load theme from {}: {error}; using defaults", path.display());
                Self::default()
            }
        }
    }

    pub fn syntect(&self) -> &SyntectTheme {
        &self.syntect
    }

    fn from_syntect(theme: SyntectTheme) -> Self {
        let text_primary = theme.settings.foreground.map_or(Color::Rgb(212, 221, 214), color_from_syntect);
        let background = theme.settings.background.map_or(Color::Rgb(21, 29, 31), color_from_syntect);
        let accent = theme.settings.caret.map_or(Color::Rgb(143, 188, 176), color_from_syntect);
        let text_secondary = blend(text_primary, background, 60);
        let sidebar_bg = blend(background, text_primary, 95);
        let heading = scope_color(&theme, "markup.heading").unwrap_or(accent);
        let link = scope_color(&theme, "markup.underline.link").unwrap_or(accent);
        let blockquote = scope_color(&theme, "markup.quote").unwrap_or(text_secondary);
        let muted = scope_color(&theme, "markup.list.bullet").unwrap_or(text_secondary);
        let success =
            scope_color(&theme, "markup.inserted").or_else(|| scope_color(&theme, "string")).unwrap_or(accent);
        let warning = scope_color(&theme, "constant.numeric").unwrap_or(accent);
        let error = scope_color(&theme, "markup.deleted").or_else(|| scope_color(&theme, "invalid")).unwrap_or(accent);
        let info = scope_color(&theme, "entity.name.function").unwrap_or(accent);
        let inline_code_foreground = scope_color(&theme, "markup.inline.raw.string.markdown").unwrap_or(text_primary);
        let inline_code_background = background;
        let diff_added_fg = scope_color(&theme, "markup.inserted.diff").unwrap_or(success);
        let diff_removed_fg = scope_color(&theme, "markup.deleted.diff").unwrap_or(error);

        Self {
            text_primary,
            text_secondary,
            background,
            sidebar_bg,
            accent,
            heading,
            link,
            blockquote,
            code_fg: inline_code_foreground,
            code_bg: inline_code_background,
            success,
            warning,
            error,
            info,
            muted,
            diff_added_fg,
            diff_added_bg: darken(diff_added_fg),
            diff_removed_fg,
            diff_removed_bg: darken(diff_removed_fg),
            syntect: Arc::new(theme),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        let themes = two_face::theme::extra();
        Self::from_syntect(themes.get(EmbeddedThemeName::Nord).clone())
    }
}

fn scope_color(theme: &SyntectTheme, scope: &str) -> Option<Color> {
    let scope = Scope::new(scope).ok()?;
    let resolved = Highlighter::new(theme).style_for_stack(&[scope]).foreground;
    let default = theme.settings.foreground?;
    (resolved != default).then(|| color_from_syntect(resolved))
}

fn color_from_syntect(color: syntect::highlighting::Color) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

fn darken(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(r.saturating_mul(3) / 10, g.saturating_mul(3) / 10, b.saturating_mul(3) / 10),
        other => other,
    }
}

fn blend(first: Color, second: Color, first_percent: u16) -> Color {
    match (first, second) {
        (Color::Rgb(fr, fg, fb), Color::Rgb(sr, sg, sb)) => {
            let mix = |a: u8, b: u8| {
                let value = (u16::from(a) * first_percent + u16::from(b) * (100 - first_percent)) / 100;
                u8::try_from(value).unwrap_or(u8::MAX)
            };
            Color::Rgb(mix(fr, sr), mix(fg, sg), mix(fb, sb))
        }
        (color, _) => color,
    }
}
