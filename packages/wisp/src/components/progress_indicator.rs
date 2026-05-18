use tui::BRAILLE_FRAMES as FRAMES;
use tui::{FitOptions, Frame, Line, Style, ViewContext};

const MESSAGES: &[&str] = &[
    "Tip: Hit Tab to adjust reasoning level (off → low → medium → high)",
    "Tip: Hit Shift+Tab to cycle through agents defined in your settings.json file",
    "Tip: Press @ to attach files to your prompt",
    "Tip: Type / to open the command picker",
    "Tip: Use /resume to pick up a previous session",
    "Tip: Wisp supports custom themes — drop a .tmTheme in ~/.wisp/themes/",
    "Tip: Open /settings to change your model, theme, or view MCP server status",
    "Tip: The context gauge in the status bar shows current context usage against the model limit",
];

/// Renders a spinner with "(esc to interrupt)" when the agent is busy.
/// Visible whenever we're waiting for a response OR tools are actively running.
#[derive(Default)]
pub struct ProgressIndicator {
    tools_running: bool,
    waiting_for_response: bool,
    tick: u16,
    was_active: bool,
    turn_count: usize,
}

impl ProgressIndicator {
    pub fn update(&mut self, completed: usize, total: usize, waiting_for_response: bool) {
        let previously_active = self.was_active;
        self.tools_running = total > 0 && completed < total;
        self.waiting_for_response = waiting_for_response;
        let now_active = self.is_active();
        self.was_active = now_active;
        if !previously_active && now_active {
            self.turn_count += 1;
        }
    }

    #[cfg(test)]
    pub fn set_tick(&mut self, tick: u16) {
        self.tick = tick;
    }

    #[cfg(test)]
    pub fn set_turn_count(&mut self, count: usize) {
        self.turn_count = count;
    }

    fn is_active(&self) -> bool {
        self.tools_running || self.waiting_for_response
    }

    fn current_message(&self) -> &'static str {
        self.turn_count.checked_sub(1).and_then(|i| MESSAGES.get(i)).copied().unwrap_or("Working...")
    }

    /// Advance the animation state. Call this on tick events.
    pub fn on_tick(&mut self) {
        if self.is_active() {
            self.tick = self.tick.wrapping_add(1);
        }
    }
}

impl ProgressIndicator {
    pub fn render(&self, context: &ViewContext) -> Frame {
        if !self.is_active() {
            return Frame::empty();
        }

        let frame_char = FRAMES[self.tick as usize % FRAMES.len()];
        let mut line = Line::default();
        line.push_styled(frame_char.to_string(), context.theme.info());
        line.push_styled(format!(" {}", self.current_message()), context.theme.text_secondary());
        line.push_with_style("  (esc to interrupt)".to_string(), Style::fg(context.theme.muted()).italic());

        let lines = vec![Line::default(), line, Line::default()];
        Frame::new(lines).fit(context.size.width, FitOptions::wrap())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> ViewContext {
        // Wide enough for the longest tip + " (esc to interrupt)" suffix to fit on one row.
        ViewContext::new((200, 24))
    }

    #[test]
    fn renders_nothing_when_idle() {
        let indicator = ProgressIndicator::default();
        assert!(indicator.render(&ctx()).lines().is_empty());
    }

    #[test]
    fn renders_nothing_when_all_complete_and_not_waiting() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(3, 3, false);
        assert!(indicator.render(&ctx()).lines().is_empty());
    }

    #[test]
    fn renders_when_tools_running() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(1, 3, false);
        let frame = indicator.render(&ctx());
        let lines = frame.lines();
        assert_eq!(lines.len(), 3);
        let text = lines[1].plain_text();
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn renders_when_waiting_for_response_without_tools() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(0, 0, true);
        let frame = indicator.render(&ctx());
        let lines = frame.lines();
        assert_eq!(lines.len(), 3);
        let text = lines[1].plain_text();
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn spinner_animates_with_tick() {
        let mut a = ProgressIndicator::default();
        a.update(0, 1, false);
        let mut b = ProgressIndicator::default();
        b.update(0, 1, false);
        b.set_tick(1);
        let text_a = a.render(&ctx()).lines()[1].plain_text();
        let text_b = b.render(&ctx()).lines()[1].plain_text();
        assert_ne!(text_a, text_b);
    }

    #[test]
    fn on_tick_advances_when_running() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(1, 3, false);
        indicator.on_tick();
        let frame = indicator.render(&ctx());
        assert!(!frame.lines().is_empty());
    }

    #[test]
    fn on_tick_advances_when_waiting() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(0, 0, true);
        let frame_before = indicator.tick;
        indicator.on_tick();
        assert_ne!(indicator.tick, frame_before);
    }

    #[test]
    fn on_tick_noop_when_idle() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(3, 3, false);
        indicator.on_tick();
        assert!(indicator.render(&ctx()).lines().is_empty());
    }

    #[test]
    fn first_turn_shows_first_tip() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(0, 0, true);
        indicator.set_turn_count(1);
        let frame = indicator.render(&ctx());
        let text = frame.lines()[1].plain_text();
        assert!(text.contains(MESSAGES[0]));
    }

    #[test]
    fn tip_advances_each_turn() {
        let mut indicator = ProgressIndicator::default();
        // First turn: inactive → active
        indicator.update(0, 0, true);
        assert_eq!(indicator.turn_count, 1);
        let tip_0 = indicator.render(&ctx()).lines()[1].plain_text();

        // Go inactive
        indicator.update(0, 0, false);

        // Second turn: inactive → active
        indicator.update(0, 0, true);
        assert_eq!(indicator.turn_count, 2);
        let tip_1 = indicator.render(&ctx()).lines()[1].plain_text();

        assert_ne!(tip_0, tip_1);
        assert!(tip_0.contains(MESSAGES[0]));
        assert!(tip_1.contains(MESSAGES[1]));
    }

    #[test]
    fn shows_working_after_tips_exhausted() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(0, 0, true);
        indicator.set_turn_count(MESSAGES.len() + 1);
        let text = indicator.render(&ctx()).lines()[1].plain_text();
        assert!(text.contains("Working..."));
    }

    #[test]
    fn reset_restarts_tips() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(0, 0, true);
        assert_eq!(indicator.turn_count, 1);

        let indicator = ProgressIndicator::default();
        assert_eq!(indicator.turn_count, 0);
    }

    #[test]
    fn staying_active_does_not_advance_tip() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(0, 0, true);
        assert_eq!(indicator.turn_count, 1);

        // Multiple updates while staying active
        indicator.update(1, 3, true);
        indicator.update(2, 3, true);
        indicator.update(3, 3, true);
        // Still waiting_for_response so still active
        assert_eq!(indicator.turn_count, 1);
    }
}
