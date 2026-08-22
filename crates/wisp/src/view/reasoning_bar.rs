use crate::theme::Theme;
use ratatui::style::Color;
use utils::ReasoningEffort;

/// Renders a compact reasoning effort bar labeled with the current effort.
///
/// Visual mapping (e.g. `levels = [Low, Medium, High]`):
/// - `None` => `none [···]` (all empty)
/// - `Low` => `low [■··]` (1 filled)
/// - `Medium` => `medium [■■·]` (2 filled)
/// - `High` => `high [■■■]` (3 filled)
pub(crate) fn reasoning_bar(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort], label_width: usize) -> String {
    let label = effort.map_or("none", ReasoningEffort::as_str);
    format!("{label:>label_width$} {}", slot_bar(filled_slots(effort, levels), levels.len()))
}

/// The width the aligned bar occupies in a column, 0 when there are no levels.
///
/// The width of the aligned bar itself, so the column math lives in one place.
pub(crate) fn reasoning_column_width(levels: &[ReasoningEffort]) -> usize {
    if levels.is_empty() { 0 } else { reasoning_bar(None, levels, reasoning_label_width(levels)).len() }
}

/// Widest label these levels can show, including "none".
pub(crate) fn reasoning_label_width(levels: &[ReasoningEffort]) -> usize {
    levels.iter().map(|level| level.as_str().len()).chain(std::iter::once("none".len())).max().unwrap_or(0)
}

/// The appropriate theme color for the given reasoning effort: `Max` is
/// highlighted as a warning, the rest step through secondary/info/success by
/// how full the bar is.
pub(crate) fn reasoning_color(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort], theme: &Theme) -> Color {
    let filled = filled_slots(effort, levels);
    let total = levels.len();
    if total == 0 {
        theme.text_secondary
    } else if effort == Some(ReasoningEffort::Max) {
        theme.warning
    } else if filled * 3 <= total {
        theme.text_secondary
    } else if filled * 3 <= total * 2 {
        theme.info
    } else {
        theme.success
    }
}

fn filled_slots(effort: Option<ReasoningEffort>, levels: &[ReasoningEffort]) -> usize {
    effort.map_or(0, |effort| levels.iter().filter(|&&level| level <= effort).count())
}

/// A `[■■·]` gauge with `filled` of `total` slots lit.
pub(crate) fn slot_bar(filled: usize, total: usize) -> String {
    let slots: String = (0..total).map(|index| if index < filled { '■' } else { '·' }).collect();
    format!("[{slots}]")
}
