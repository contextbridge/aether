use tui::{Color, Theme};

/// Nominal context usage values shown in the status line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextUsageDisplay {
    pub used_tokens: u32,
    pub limit_tokens: u32,
}

impl ContextUsageDisplay {
    pub fn new(used_tokens: u32, limit_tokens: u32) -> Self {
        Self { used_tokens, limit_tokens }
    }

    pub fn used_ratio(&self) -> f64 {
        if self.limit_tokens == 0 {
            return 0.0;
        }
        (f64::from(self.used_tokens) / f64::from(self.limit_tokens)).clamp(0.0, 1.0)
    }
}

/// Renders a bracketed slot bar with `filled` of `total` slots filled.
///
/// Example: `slot_bar(2, 3)` => `[■■·]`
pub(crate) fn slot_bar(filled: usize, total: usize) -> String {
    let slots: String = (0..total).map(|i| if i < filled { '■' } else { '·' }).collect();
    format!("[{slots}]")
}

/// Renders a compact 3-slot context gauge with label and nominal usage.
///
/// Each slot represents ~33% of context used. Visual mapping:
/// - `200k / 200k` => `ctx [■■■] 200k / 200k`
/// - `100k / 200k` => `ctx [■■·] 100k / 200k`
/// - `1.2k / 200k` => `ctx [···] 1.2k / 200k`
pub(crate) fn context_bar(usage: ContextUsageDisplay) -> String {
    const TOTAL: u32 = 3;
    let filled = (usage.used_tokens.saturating_mul(TOTAL) + usage.limit_tokens / 2) / usage.limit_tokens.max(1);
    let filled = (filled as usize).min(TOTAL as usize);
    format!(
        "ctx {} {} / {}",
        slot_bar(filled, TOTAL as usize),
        format_tokens(usage.used_tokens),
        format_tokens(usage.limit_tokens)
    )
}

/// Returns the appropriate theme color for the given context usage.
///
/// Tiers (used capacity):
/// - `<=70%` used → `text_secondary` (subtle awareness)
/// - `71-85%` used → `warning` (yellow)
/// - `>=86%` used → `error` (red, attention needed)
pub(crate) fn context_color(usage: ContextUsageDisplay, theme: &Theme) -> Color {
    let used_pct = usage.used_ratio() * 100.0;
    if used_pct >= 86.0 {
        theme.error()
    } else if used_pct >= 71.0 {
        theme.warning()
    } else {
        theme.text_secondary()
    }
}

/// Formats a token count compactly: `999`, `1k`, `1.2k`, `12k`, `150k`, `1.2M`.
pub(crate) fn format_tokens(n: u32) -> String {
    match n {
        n if n < 1_000 => n.to_string(),
        n if n < 1_000_000 => format_with_unit(f64::from(n) / 1_000.0, "k"),
        n => format_with_unit(f64::from(n) / 1_000_000.0, "M"),
    }
}

fn format_with_unit(value: f64, unit: &str) -> String {
    let rounded_one = (value * 10.0).round() / 10.0;
    if (rounded_one - rounded_one.trunc()).abs() < f64::EPSILON {
        format!("{rounded_one:.0}{unit}")
    } else {
        format!("{rounded_one:.1}{unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(used: u32, limit: u32) -> ContextUsageDisplay {
        ContextUsageDisplay::new(used, limit)
    }

    #[test]
    fn slot_bar_empty() {
        assert_eq!(slot_bar(0, 3), "[···]");
    }

    #[test]
    fn slot_bar_partial() {
        assert_eq!(slot_bar(2, 3), "[■■·]");
    }

    #[test]
    fn slot_bar_full() {
        assert_eq!(slot_bar(3, 3), "[■■■]");
    }

    #[test]
    fn bar_full() {
        assert_eq!(context_bar(usage(200_000, 200_000)), "ctx [■■■] 200k / 200k");
    }

    #[test]
    fn bar_empty() {
        assert_eq!(context_bar(usage(0, 200_000)), "ctx [···] 0 / 200k");
    }

    #[test]
    fn bar_low() {
        assert_eq!(context_bar(usage(1_200, 200_000)), "ctx [···] 1.2k / 200k");
    }

    #[test]
    fn bar_half() {
        assert_eq!(context_bar(usage(100_000, 200_000)), "ctx [■■·] 100k / 200k");
    }

    #[test]
    fn bar_near_full() {
        assert_eq!(context_bar(usage(190_000, 200_000)), "ctx [■■■] 190k / 200k");
    }

    #[test]
    fn bar_fills_with_usage() {
        let empty = context_bar(usage(0, 200_000));
        let half = context_bar(usage(100_000, 200_000));
        let full = context_bar(usage(200_000, 200_000));
        assert!(empty.contains("[···]"));
        assert!(half.contains("[■■·]"));
        assert!(full.contains("[■■■]"));
    }

    #[test]
    fn color_tiers() {
        let theme = Theme::default();
        assert_eq!(context_color(usage(0, 200_000), &theme), theme.text_secondary());
        assert_eq!(context_color(usage(140_000, 200_000), &theme), theme.text_secondary());
        assert_eq!(context_color(usage(142_000, 200_000), &theme), theme.warning());
        assert_eq!(context_color(usage(170_000, 200_000), &theme), theme.warning());
        assert_eq!(context_color(usage(172_000, 200_000), &theme), theme.error());
        assert_eq!(context_color(usage(200_000, 200_000), &theme), theme.error());
    }

    #[test]
    fn format_tokens_examples() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_000), "1k");
        assert_eq!(format_tokens(1_200), "1.2k");
        assert_eq!(format_tokens(12_000), "12k");
        assert_eq!(format_tokens(150_000), "150k");
        assert_eq!(format_tokens(1_200_000), "1.2M");
    }
}
