use super::{Color, Theme, lighten_color};

impl Theme {
    /// A minimal light theme.
    #[allow(dead_code)]
    pub fn light() -> Self {
        const FG: Color = Color::Rgb { r: 0x22, g: 0x22, b: 0x22 };
        const BG: Color = Color::Rgb { r: 0xFA, g: 0xFA, b: 0xFA };
        const ACCENT: Color = Color::Rgb { r: 0x00, g: 0x66, b: 0xCC };
        const GREEN: Color = Color::Rgb { r: 0x22, g: 0x88, b: 0x22 };
        const RED: Color = Color::Rgb { r: 0xCC, g: 0x22, b: 0x22 };
        const ORANGE: Color = Color::Rgb { r: 0xCC, g: 0x66, b: 0x00 };

        Theme::builder()
            .fg(FG)
            .bg(BG)
            .accent(ACCENT)
            .highlight_bg(Color::Rgb { r: 0xDD, g: 0xDD, b: 0xDD })
            .highlight_fg(FG)
            .text_secondary(Color::Rgb { r: 0x66, g: 0x66, b: 0x66 })
            .code_fg(Color::Rgb { r: 0x33, g: 0x66, b: 0x33 })
            .code_bg(Color::Rgb { r: 0xF0, g: 0xF0, b: 0xF0 })
            .heading(ACCENT)
            .link(Color::Rgb { r: 0x00, g: 0x44, b: 0xAA })
            .blockquote(Color::Rgb { r: 0x66, g: 0x44, b: 0x88 })
            .muted(Color::Rgb { r: 0x88, g: 0x88, b: 0x88 })
            .success(GREEN)
            .warning(ORANGE)
            .error(RED)
            .info(ACCENT)
            .secondary(Color::Rgb { r: 0x66, g: 0x33, b: 0x99 })
            .sidebar_bg(Color::Rgb { r: 0xF4, g: 0xF4, b: 0xF8 })
            .diff_added_fg(GREEN)
            .diff_removed_fg(RED)
            .diff_added_bg(lighten_color(GREEN))
            .diff_removed_bg(lighten_color(RED))
            .build()
            .expect("built-in light theme has all fields")
    }
}
