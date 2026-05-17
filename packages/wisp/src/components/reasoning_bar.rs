use super::context_bar::slot_bar;
use tui::{Color, Theme};
use utils::ReasoningEffort;

fn filled_slots(effort: Option<ReasoningEffort>, total: usize) -> usize {
    effort.map_or(0, |e| e.ordinal() + 1).min(total)
}

/// Renders a compact reasoning effort bar labeled with the current effort.
///
/// Visual mapping (e.g. `total_levels = 3`):
/// - `None` => `none [···]` (all empty)
/// - `Low` => `low [■··]` (1 filled)
/// - `Medium` => `medium [■■·]` (2 filled)
/// - `High` => `high [■■■]` (3 filled)
pub(crate) fn reasoning_bar(effort: Option<ReasoningEffort>, total_levels: usize) -> String {
    let label = effort.map_or("none", ReasoningEffort::as_str);
    format!("{label} {}", slot_bar(filled_slots(effort, total_levels), total_levels))
}

/// Returns the appropriate theme color for the given reasoning effort.
///
/// Uses ratio-based thresholds:
/// - filled ≤ 1/total  → `text_secondary` (subdued)
/// - filled ≤ 2/3 of total → `info`
/// - above            → `success`
pub(crate) fn reasoning_color(effort: Option<ReasoningEffort>, total_levels: usize, theme: &Theme) -> Color {
    let filled = filled_slots(effort, total_levels);
    if total_levels == 0 || filled * 3 <= total_levels {
        theme.text_secondary()
    } else if filled * 3 <= total_levels * 2 {
        theme.info()
    } else {
        theme.success()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_none_3_slots() {
        assert_eq!(reasoning_bar(None, 3), "none [···]");
    }

    #[test]
    fn bar_low_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Low), 3), "low [■··]");
    }

    #[test]
    fn bar_medium_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Medium), 3), "medium [■■·]");
    }

    #[test]
    fn bar_high_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::High), 3), "high [■■■]");
    }

    #[test]
    fn bar_4_slots() {
        assert_eq!(reasoning_bar(None, 4), "none [····]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Low), 4), "low [■···]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::High), 4), "high [■■■·]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Xhigh), 4), "xhigh [■■■■]");
    }

    #[test]
    fn bar_xhigh_clamped_to_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Xhigh), 3), "xhigh [■■■]");
    }

    #[test]
    fn color_tiers_3_slots() {
        let theme = Theme::default();
        assert_eq!(reasoning_color(None, 3, &theme), theme.text_secondary());
        assert_eq!(reasoning_color(Some(ReasoningEffort::Low), 3, &theme), theme.text_secondary());
        assert_eq!(reasoning_color(Some(ReasoningEffort::Medium), 3, &theme), theme.info());
        assert_eq!(reasoning_color(Some(ReasoningEffort::High), 3, &theme), theme.success());
    }

    #[test]
    fn color_tiers_4_slots() {
        let theme = Theme::default();
        // 4 slots: filled=1 → 1*3=3 ≤ 4 → secondary
        assert_eq!(reasoning_color(Some(ReasoningEffort::Low), 4, &theme), theme.text_secondary());
        // filled=2 → 2*3=6 ≤ 8 → info
        assert_eq!(reasoning_color(Some(ReasoningEffort::Medium), 4, &theme), theme.info());
        // filled=3 → 3*3=9 > 8 → success
        assert_eq!(reasoning_color(Some(ReasoningEffort::High), 4, &theme), theme.success());
        // filled=4 → success
        assert_eq!(reasoning_color(Some(ReasoningEffort::Xhigh), 4, &theme), theme.success());
    }
}
