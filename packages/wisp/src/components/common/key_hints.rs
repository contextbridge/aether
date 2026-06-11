use tui::{Line, Style, Theme};

/// Renders a footer hint line from `(key, action)` pairs: each key in the accent
/// color followed by its muted action label, with two spaces between pairs.
pub(crate) fn render_key_hints(theme: &Theme, keys: &[(&str, &str)]) -> Line {
    let mut line = Line::default();
    for (index, (key, action)) in keys.iter().enumerate() {
        if index > 0 {
            line.push_text("  ");
        }
        line.push_with_style(*key, Style::fg(theme.accent()));
        line.push_text(" ");
        line.push_with_style(*action, Style::fg(theme.muted()));
    }
    line
}
