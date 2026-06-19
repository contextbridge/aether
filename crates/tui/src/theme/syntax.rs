use super::{Color, Theme, darken_color};
use std::path::Path;
use std::sync::Arc;

/// Parse the embedded Sage `.tmTheme` into a syntect theme.
///
/// Called once at `Theme` construction time; the result is cached in
/// `Theme::syntect_theme`.
pub(super) fn parse_default_syntect_theme() -> syntect::highlighting::Theme {
    let cursor = std::io::Cursor::new(include_bytes!("../../assets/sage.tmTheme"));
    syntect::highlighting::ThemeSet::load_from_reader(&mut std::io::BufReader::new(cursor))
        .expect("embedded sage.tmTheme is valid")
}

const DEFAULT_FG: Color = Color::Rgb { r: 0xBF, g: 0xBD, b: 0xB6 };
const DEFAULT_BG: Color = Color::Rgb { r: 0x1E, g: 0x1E, b: 0x2E };
const DEFAULT_CODE_BG: Color = Color::Rgb { r: 40, g: 40, b: 40 };
const DEFAULT_ACCENT: Color = Color::Rgb { r: 255, g: 215, b: 0 };
const DEFAULT_HIGHLIGHT_BG: Color = Color::Rgb { r: 0x1a, g: 0x4a, b: 0x50 };

impl Theme {
    /// Return the cached syntect theme for syntax highlighting.
    pub fn syntect_theme(&self) -> &syntect::highlighting::Theme {
        &self.syntect_theme
    }

    /// Load theme from a `.tmTheme` file.
    pub fn load_from_path(path: &Path) -> Self {
        use syntect::highlighting::ThemeSet;
        use tracing::warn;

        match ThemeSet::get_theme(path) {
            Ok(syntect_theme) => Self::from(&syntect_theme),
            Err(e) => {
                warn!("Failed to load theme from {}: {e}. Falling back to defaults.", path.display());
                Self::default()
            }
        }
    }
}

impl From<&syntect::highlighting::Theme> for Theme {
    #[allow(clippy::similar_names)]
    fn from(syntect: &syntect::highlighting::Theme) -> Self {
        let syntect_bg =
            syntect.settings.background.unwrap_or(syntect::highlighting::Color { r: 0x1E, g: 0x1E, b: 0x2E, a: 0xFF });

        let accent = syntect.settings.caret.map_or(DEFAULT_ACCENT, color_from_syntect);

        let text_secondary = derive_text_secondary(syntect);

        let heading = resolve_scope_fg(syntect, "markup.heading.markdown")
            .or_else(|| resolve_scope_fg(syntect, "markup.heading"))
            .unwrap_or(accent);

        let link = resolve_scope_fg(syntect, "markup.underline.link")
            .or_else(|| resolve_scope_fg(syntect, "markup.link"))
            .unwrap_or(accent);

        let blockquote = resolve_scope_fg(syntect, "markup.quote").unwrap_or(text_secondary);

        let muted = resolve_scope_fg(syntect, "markup.list.bullet")
            .or_else(|| syntect.settings.gutter_foreground.map(|c| composite_over(c, syntect_bg)))
            .unwrap_or(text_secondary);

        let fg = syntect.settings.foreground.map_or(DEFAULT_FG, color_from_syntect);

        let inline_code_fg = resolve_scope_fg(syntect, "markup.inline.raw.string.markdown")
            .or_else(|| resolve_scope_fg(syntect, "markup.raw"))
            .unwrap_or(fg);

        let error = resolve_scope_fg(syntect, "markup.deleted")
            .or_else(|| resolve_scope_fg(syntect, "markup.deleted.diff"))
            .or_else(|| resolve_scope_fg(syntect, "invalid"))
            .unwrap_or(accent);

        let warning = resolve_scope_fg(syntect, "constant.numeric").unwrap_or(accent);

        let success = resolve_scope_fg(syntect, "markup.inserted")
            .or_else(|| resolve_scope_fg(syntect, "markup.inserted.diff"))
            .or_else(|| resolve_scope_fg(syntect, "string"))
            .unwrap_or(accent);

        let info = resolve_scope_fg(syntect, "entity.name.function")
            .or_else(|| resolve_scope_fg(syntect, "support.function"))
            .unwrap_or(accent);

        let secondary = resolve_scope_fg(syntect, "keyword")
            .or_else(|| resolve_scope_fg(syntect, "storage.type"))
            .unwrap_or(accent);

        let (bg, highlight_bg, highlight_fg, inline_code_bg) = resolve_bg_colors(syntect, syntect_bg, fg);

        let sidebar_bg = nudge_toward_fg(bg, fg);

        let diff_added_fg = resolve_scope_fg(syntect, "markup.inserted.diff")
            .or_else(|| resolve_scope_fg(syntect, "markup.inserted"))
            .or_else(|| resolve_scope_fg(syntect, "string"))
            .unwrap_or(accent);

        let diff_removed_fg = resolve_scope_fg(syntect, "markup.deleted.diff")
            .or_else(|| resolve_scope_fg(syntect, "markup.deleted"))
            .unwrap_or(accent);

        Self {
            fg,
            bg,
            accent,
            highlight_bg,
            highlight_fg,
            text_secondary,
            code_fg: inline_code_fg,
            code_bg: inline_code_bg,
            heading,
            link,
            blockquote,
            muted,
            success,
            warning,
            error,
            info,
            secondary,
            sidebar_bg,
            diff_added_fg,
            diff_removed_fg,
            diff_added_bg: darken_color(diff_added_fg),
            diff_removed_bg: darken_color(diff_removed_fg),
            syntect_theme: Arc::new(syntect.clone()),
        }
    }
}

#[allow(clippy::similar_names)]
fn resolve_bg_colors(
    syntect: &syntect::highlighting::Theme,
    syntect_bg: syntect::highlighting::Color,
    fg: Color,
) -> (Color, Color, Color, Color) {
    let bg = syntect.settings.background.map_or(DEFAULT_BG, color_from_syntect);

    let highlight_bg = syntect
        .settings
        .line_highlight
        .or(syntect.settings.selection)
        .map_or(DEFAULT_HIGHLIGHT_BG, |c| composite_over(c, syntect_bg));

    let highlight_fg = syntect.settings.selection_foreground.map_or(fg, color_from_syntect);

    let inline_code_bg = syntect.settings.background.map_or(DEFAULT_CODE_BG, color_from_syntect);

    (bg, highlight_bg, highlight_fg, inline_code_bg)
}

/// Resolve the foreground color for a scope string against the theme.
fn resolve_scope_fg(theme: &syntect::highlighting::Theme, scope_str: &str) -> Option<Color> {
    use syntect::highlighting::Highlighter;
    use syntect::parsing::Scope;

    let scope = Scope::new(scope_str).ok()?;
    let highlighter = Highlighter::new(theme);
    let style = highlighter.style_for_stack(&[scope]);

    let resolved = style.foreground;
    let default_fg = theme.settings.foreground?;

    if resolved.r == default_fg.r && resolved.g == default_fg.g && resolved.b == default_fg.b {
        return None;
    }

    Some(color_from_syntect(resolved))
}

/// Blend the theme's foreground toward its background at ~40%.
fn derive_text_secondary(theme: &syntect::highlighting::Theme) -> Color {
    use syntect::highlighting::Color as SyntectColor;

    let fg = theme.settings.foreground.unwrap_or(SyntectColor { r: 0xBF, g: 0xBD, b: 0xB6, a: 0xFF });
    let bg = theme.settings.background.unwrap_or(SyntectColor { r: 0x28, g: 0x28, b: 0x28, a: 0xFF });

    #[allow(clippy::cast_possible_truncation)]
    let blend = |f: u8, b: u8| -> u8 { ((u16::from(f) * 60 + u16::from(b) * 40) / 100) as u8 };

    Color::Rgb { r: blend(fg.r, bg.r), g: blend(fg.g, bg.g), b: blend(fg.b, bg.b) }
}

/// Nudge a background color ~5% toward the foreground to produce a
/// subtly distinct sidebar background.
#[allow(clippy::cast_possible_truncation)]
fn nudge_toward_fg(bg: Color, fg: Color) -> Color {
    match (bg, fg) {
        (Color::Rgb { r: br, g: bg_g, b: bb }, Color::Rgb { r: fr, g: fg_g, b: fb }) => {
            let blend = |b: u8, f: u8| -> u8 { ((u16::from(b) * 95 + u16::from(f) * 5) / 100) as u8 };
            Color::Rgb { r: blend(br, fr), g: blend(bg_g, fg_g), b: blend(bb, fb) }
        }
        _ => bg,
    }
}

fn color_from_syntect(color: syntect::highlighting::Color) -> Color {
    Color::Rgb { r: color.r, g: color.g, b: color.b }
}

/// Alpha-composite `fg` over `bg`, producing an opaque `Color`.
///
/// Many `.tmTheme` colors (e.g. `lineHighlight`, `selection`) use alpha to
/// create subtle overlays. Since terminals can't render alpha, we pre-blend
/// against the theme background.
#[allow(clippy::cast_possible_truncation)]
fn composite_over(fg: syntect::highlighting::Color, bg: syntect::highlighting::Color) -> Color {
    let a = u16::from(fg.a);
    let blend = |f: u8, b: u8| -> u8 { ((u16::from(f) * a + u16::from(b) * (255 - a)) / 255) as u8 };
    Color::Rgb { r: blend(fg.r, bg.r), g: blend(fg.g, bg.g), b: blend(fg.b, bg.b) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use syntect::highlighting::ThemeSettings;
    use tempfile::TempDir;

    fn bare_syntect_theme() -> syntect::highlighting::Theme {
        syntect::highlighting::Theme {
            name: Some("Bare".into()),
            author: None,
            settings: ThemeSettings {
                foreground: Some(syntect::highlighting::Color { r: 0xCC, g: 0xCC, b: 0xCC, a: 0xFF }),
                background: Some(syntect::highlighting::Color { r: 0x11, g: 0x11, b: 0x11, a: 0xFF }),
                caret: Some(syntect::highlighting::Color { r: 0xAA, g: 0xBB, b: 0xCC, a: 0xFF }),
                ..ThemeSettings::default()
            },
            scopes: Vec::new(),
        }
    }

    const LOADABLE_TMTHEME: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>name</key>
    <string>Loadable</string>
    <key>settings</key>
    <array>
        <dict>
            <key>settings</key>
            <dict>
                <key>foreground</key>
                <string>#112233</string>
                <key>background</key>
                <string>#000000</string>
                <key>selection</key>
                <string>#334455</string>
            </dict>
        </dict>
    </array>
</dict>
</plist>"#;

    #[test]
    fn bare_theme_falls_back_to_accent() {
        let accent = Color::Rgb { r: 0xAA, g: 0xBB, b: 0xCC };
        let syntect = bare_syntect_theme();
        let theme = Theme::from(&syntect);

        assert_eq!(theme.heading(), accent);
        assert_eq!(theme.link(), accent);
        assert_eq!(theme.error(), accent);
        assert_eq!(theme.warning(), accent);
        assert_eq!(theme.success(), accent);
        assert_eq!(theme.info(), accent);
        assert_eq!(theme.secondary(), accent);
        assert_eq!(theme.diff_added_fg(), accent);
        assert_eq!(theme.diff_removed_fg(), accent);
    }

    #[test]
    fn valid_theme_file_loads_from_path() {
        let temp_dir = TempDir::new().unwrap();
        let theme_path = temp_dir.path().join("custom.tmTheme");
        fs::write(&theme_path, LOADABLE_TMTHEME).unwrap();

        let loaded = Theme::load_from_path(&theme_path);

        assert_eq!(loaded.text_primary(), Color::Rgb { r: 0x11, g: 0x22, b: 0x33 });
    }

    #[test]
    fn loaded_theme_preserves_syntect_theme_when_cloned() {
        let temp_dir = TempDir::new().unwrap();
        let theme_path = temp_dir.path().join("custom.tmTheme");
        fs::write(&theme_path, LOADABLE_TMTHEME).unwrap();

        let loaded = Theme::load_from_path(&theme_path);
        let cloned = loaded.clone();
        let syntect = cloned.syntect_theme();

        assert_eq!(
            syntect.settings.foreground,
            Some(syntect::highlighting::Color { r: 0x11, g: 0x22, b: 0x33, a: 0xFF })
        );
        assert_eq!(
            syntect.settings.selection,
            Some(syntect::highlighting::Color { r: 0x33, g: 0x44, b: 0x55, a: 0xFF })
        );
    }

    #[test]
    fn highlight_bg_prefers_line_highlight_over_selection() {
        let mut syntect = bare_syntect_theme();
        syntect.settings.line_highlight = Some(syntect::highlighting::Color { r: 0x31, g: 0x32, b: 0x44, a: 0xFF });
        syntect.settings.selection = Some(syntect::highlighting::Color { r: 0x99, g: 0x99, b: 0x99, a: 0x40 });

        let theme = Theme::from(&syntect);

        assert_eq!(theme.highlight_bg(), Color::Rgb { r: 0x31, g: 0x32, b: 0x44 });
    }

    #[test]
    fn highlight_bg_falls_back_to_selection_without_line_highlight() {
        let mut syntect = bare_syntect_theme();
        syntect.settings.line_highlight = None;
        syntect.settings.selection = Some(syntect::highlighting::Color { r: 0x33, g: 0x44, b: 0x55, a: 0xFF });

        let theme = Theme::from(&syntect);

        assert_eq!(theme.highlight_bg(), Color::Rgb { r: 0x33, g: 0x44, b: 0x55 });
    }

    #[test]
    fn highlight_bg_composites_alpha_over_background() {
        // Kiwi-like: lineHighlight=#00000050 over background=#212121
        let mut syntect = bare_syntect_theme();
        syntect.settings.background = Some(syntect::highlighting::Color { r: 0x21, g: 0x21, b: 0x21, a: 0xFF });
        syntect.settings.line_highlight = Some(syntect::highlighting::Color { r: 0x00, g: 0x00, b: 0x00, a: 0x50 });

        let theme = Theme::from(&syntect);

        // 0x50/0xFF ≈ 31.4% opacity: blend(0x00, 0x21) = (0*80 + 33*175)/255 ≈ 22 = 0x16
        let expected = Color::Rgb { r: 0x16, g: 0x16, b: 0x16 };
        assert_eq!(theme.highlight_bg(), expected);
    }

    #[test]
    fn muted_composites_gutter_foreground_alpha() {
        // Aster-like: gutterForeground=#4f4f5e90 over background=#1a1a2e
        let mut syntect = bare_syntect_theme();
        syntect.settings.background = Some(syntect::highlighting::Color { r: 0x1A, g: 0x1A, b: 0x2E, a: 0xFF });
        syntect.settings.gutter_foreground = Some(syntect::highlighting::Color { r: 0x4F, g: 0x4F, b: 0x5E, a: 0x90 });
        // No markup.list.bullet scope, so muted falls back to gutter_foreground
        let theme = Theme::from(&syntect);

        // blend(0x4F, 0x1A) = (0x4F*0x90 + 0x1A*(255-0x90)) / 255
        #[allow(clippy::cast_possible_truncation)]
        let blend = |f: u16, b: u16| -> u8 { ((f * 0x90 + b * (255 - 0x90)) / 255) as u8 };
        let expected = Color::Rgb { r: blend(0x4F, 0x1A), g: blend(0x4F, 0x1A), b: blend(0x5E, 0x2E) };
        assert_eq!(theme.muted(), expected);
    }

    #[test]
    fn malformed_theme_falls_back_to_default() {
        let temp_dir = TempDir::new().unwrap();
        let theme_path = temp_dir.path().join("broken.tmTheme");
        fs::write(&theme_path, "not valid xml").unwrap();

        let loaded = Theme::load_from_path(&theme_path);

        let default = Theme::default();
        assert_eq!(loaded.primary(), default.primary());
        assert_eq!(loaded.code_bg(), default.code_bg());
    }
}
