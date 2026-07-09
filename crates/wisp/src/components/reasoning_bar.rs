use super::context_bar::slot_bar;
use tui::{Color, Theme};
use utils::ReasoningEffort;

fn filled_slots(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> usize {
    effort.map_or(0, |effort| levels.iter().filter(|&&level| level <= effort).count())
}

/// Renders a compact reasoning effort bar labeled with the current effort.
///
/// Visual mapping (e.g. `total_levels = 3`):
/// - `None` => `none [···]` (all empty)
/// - `Low` => `low [■··]` (1 filled)
/// - `Medium` => `medium [■■·]` (2 filled)
/// - `High` => `high [■■■]` (3 filled)
pub(crate) fn reasoning_bar(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> String {
    let label = effort.map_or("none", ReasoningEffort::as_str);
    format!("{label} {}", slot_bar(filled_slots(effort, levels), levels.len()))
}

/// Returns the appropriate theme color for the given reasoning effort.
///
/// Uses ratio-based thresholds, with maximum effort highlighted separately:
/// - `Max` → `warning`
/// - filled ≤ 1/total → `text_secondary` (subdued)
/// - filled ≤ 2/3 of total → `info`
/// - above → `success`
pub(crate) fn reasoning_color(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort], theme: &Theme) -> Color {
    let filled = filled_slots(effort, levels);
    let total_levels = levels.len();
    if total_levels == 0 {
        theme.text_secondary()
    } else if effort == Some(ReasoningEffort::Max) {
        theme.warning()
    } else if filled * 3 <= total_levels {
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

    const MINIMAL_LEVELS: &[ReasoningEffort] =
        &[ReasoningEffort::Minimal, ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High];
    const THREE_LEVELS: &[ReasoningEffort] = &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High];
    const FOUR_LEVELS: &[ReasoningEffort] =
        &[ReasoningEffort::Low, ReasoningEffort::Medium, ReasoningEffort::High, ReasoningEffort::Xhigh];

    #[test]
    fn bar_uses_positions_within_models_supported_levels() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Minimal), MINIMAL_LEVELS), "minimal [■···]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Low), MINIMAL_LEVELS), "low [■■··]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Low), THREE_LEVELS), "low [■··]");
    }

    #[test]
    fn bar_none_3_slots() {
        assert_eq!(reasoning_bar(None, THREE_LEVELS), "none [···]");
    }

    #[test]
    fn bar_low_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Low), THREE_LEVELS), "low [■··]");
    }

    #[test]
    fn bar_medium_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Medium), THREE_LEVELS), "medium [■■·]");
    }

    #[test]
    fn bar_high_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::High), THREE_LEVELS), "high [■■■]");
    }

    #[test]
    fn bar_4_slots() {
        assert_eq!(reasoning_bar(None, FOUR_LEVELS), "none [····]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Low), FOUR_LEVELS), "low [■···]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::High), FOUR_LEVELS), "high [■■■·]");
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Xhigh), FOUR_LEVELS), "xhigh [■■■■]");
    }

    #[test]
    fn bar_xhigh_clamped_to_3_slots() {
        assert_eq!(reasoning_bar(Some(ReasoningEffort::Xhigh), THREE_LEVELS), "xhigh [■■■]");
    }

    #[test]
    fn color_tiers_3_slots() {
        let theme = Theme::default();
        assert_eq!(reasoning_color(None, THREE_LEVELS, &theme), theme.text_secondary());
        assert_eq!(reasoning_color(Some(ReasoningEffort::Low), THREE_LEVELS, &theme), theme.text_secondary());
        assert_eq!(reasoning_color(Some(ReasoningEffort::Medium), THREE_LEVELS, &theme), theme.info());
        assert_eq!(reasoning_color(Some(ReasoningEffort::High), THREE_LEVELS, &theme), theme.success());
    }

    #[test]
    fn color_tiers_4_slots() {
        let theme = Theme::default();
        // 4 slots: filled=1 → 1*3=3 ≤ 4 → secondary
        assert_eq!(reasoning_color(Some(ReasoningEffort::Low), FOUR_LEVELS, &theme), theme.text_secondary());
        // filled=2 → 2*3=6 ≤ 8 → info
        assert_eq!(reasoning_color(Some(ReasoningEffort::Medium), FOUR_LEVELS, &theme), theme.info());
        // filled=3 → 3*3=9 > 8 → success
        assert_eq!(reasoning_color(Some(ReasoningEffort::High), FOUR_LEVELS, &theme), theme.success());
        // filled=4 → success
        assert_eq!(reasoning_color(Some(ReasoningEffort::Xhigh), FOUR_LEVELS, &theme), theme.success());
    }
}
