use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

pub const BRAILLE_FRAMES: [char; 10] = ['⠒', '⠮', '⠷', '⢷', '⡾', '⣯', '⣽', '⣿', '⣭', '⢯'];

const MESSAGES: &[&str] = &[
    "Tip: Hit Tab to adjust reasoning level (off → low → medium → high)",
    "Tip: Hit Shift+Tab to cycle through agents defined in your settings.json file",
    "Tip: Press @ to attach files to your prompt",
    "Tip: Type / to open the command picker",
    "Tip: Use /resume to pick up a previous session",
    "Tip: wisp-next supports custom themes — drop a .tmTheme in ~/.wisp/themes/",
    "Tip: Open /settings to change your model, theme, or view MCP server status",
    "Tip: The context gauge in the status bar shows current context usage against the model limit",
];

/// Renders a spinner with "(esc to interrupt)" when the agent is busy.
/// Visible whenever we're waiting for a response OR tools are actively running.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum WorkspaceProgress {
    #[default]
    None,
    Moving,
    LoadingSession,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProgressActivity {
    pub agent_busy: bool,
    pub workspace: WorkspaceProgress,
    pub compaction_active: bool,
}

impl ProgressActivity {
    fn display(self) -> ProgressDisplay {
        match self.workspace {
            WorkspaceProgress::Moving => ProgressDisplay::MovingWorkspace { interruptible: self.agent_busy },
            WorkspaceProgress::LoadingSession => ProgressDisplay::LoadingSession { interruptible: self.agent_busy },
            WorkspaceProgress::None if self.compaction_active => {
                ProgressDisplay::Compacting { interruptible: self.agent_busy }
            }
            WorkspaceProgress::None if self.agent_busy => ProgressDisplay::AgentWorking,
            WorkspaceProgress::None => ProgressDisplay::Idle,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ProgressDisplay {
    #[default]
    Idle,
    AgentWorking,
    Compacting {
        interruptible: bool,
    },
    MovingWorkspace {
        interruptible: bool,
    },
    LoadingSession {
        interruptible: bool,
    },
}

impl ProgressDisplay {
    fn is_active(self) -> bool {
        self != Self::Idle
    }

    fn is_interruptible(self) -> bool {
        match self {
            Self::AgentWorking => true,
            Self::Compacting { interruptible }
            | Self::MovingWorkspace { interruptible }
            | Self::LoadingSession { interruptible } => interruptible,
            Self::Idle => false,
        }
    }
}

#[derive(Default)]
pub struct ProgressIndicator {
    display: ProgressDisplay,
    tick: u16,
    agent_was_busy: bool,
    turn_count: usize,
}

impl ProgressIndicator {
    pub fn update(&mut self, activity: ProgressActivity) {
        if !self.agent_was_busy && activity.agent_busy {
            self.turn_count += 1;
        }
        self.agent_was_busy = activity.agent_busy;
        self.display = activity.display();
    }

    pub fn is_active(&self) -> bool {
        self.display.is_active()
    }

    pub fn is_interruptible(&self) -> bool {
        self.display.is_interruptible()
    }

    pub fn line_count(&self) -> usize {
        if self.is_active() { 3 } else { 0 }
    }

    /// Reset progress/tip state on context/new-session clear.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Advance the animation state. Call this on tick events.
    pub fn on_tick(&mut self) {
        if self.is_active() {
            self.tick = self.tick.wrapping_add(1);
        }
    }

    /// Render the progress indicator as padded ratatui Lines (blank, content, blank).
    /// Returns empty vec when idle.
    pub fn render(
        &self,
        info_color: Color,
        warning_color: Color,
        secondary_color: Color,
        muted_color: Color,
        padding: usize,
    ) -> Vec<Line<'static>> {
        if !self.is_active() {
            return Vec::new();
        }

        let frame_char = BRAILLE_FRAMES[self.tick as usize % BRAILLE_FRAMES.len()];
        let spinner_color =
            if matches!(self.display, ProgressDisplay::Compacting { .. }) { warning_color } else { info_color };

        let mut spans = Vec::new();
        spans.push(Span::styled(" ".repeat(padding), Style::default()));
        spans.push(Span::styled(frame_char.to_string(), Style::new().fg(spinner_color)));
        spans.push(Span::styled(format!(" {}", self.current_message()), Style::new().fg(secondary_color)));
        if self.display.is_interruptible() {
            spans.push(Span::styled(
                "  (esc to interrupt)".to_string(),
                Style::new().fg(muted_color).add_modifier(Modifier::ITALIC),
            ));
        }

        vec![Line::default(), Line::from(spans), Line::default()]
    }

    fn current_message(&self) -> &'static str {
        match self.display {
            ProgressDisplay::MovingWorkspace { .. } => "Moving workspace...",
            ProgressDisplay::LoadingSession { .. } => "Loading session in new workspace...",
            ProgressDisplay::Compacting { .. } => "Compacting context...",
            ProgressDisplay::AgentWorking => {
                self.turn_count.checked_sub(1).and_then(|i| MESSAGES.get(i)).copied().unwrap_or("Working...")
            }
            ProgressDisplay::Idle => "",
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

    #[cfg(test)]
    #[allow(private_interfaces)]
    pub fn set_display(&mut self, display: ProgressDisplay) {
        self.display = display;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INFO: Color = Color::Cyan;
    const WARNING: Color = Color::Yellow;
    const SECONDARY: Color = Color::Gray;
    const MUTED: Color = Color::DarkGray;

    fn render(indicator: &ProgressIndicator) -> Vec<Line<'static>> {
        indicator.render(INFO, WARNING, SECONDARY, MUTED, 2)
    }

    fn plain_text(lines: &[Line<'static>]) -> String {
        let mut out = String::new();
        for line in lines {
            for span in &line.spans {
                out.push_str(&span.content);
            }
        }
        out
    }

    #[test]
    fn renders_nothing_when_idle() {
        let indicator = ProgressIndicator::default();
        assert!(render(&indicator).is_empty());
    }

    #[test]
    fn renders_nothing_after_busy_clears() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.update(ProgressActivity::default());
        assert!(render(&indicator).is_empty());
    }

    #[test]
    fn renders_esc_hint_when_agent_busy() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        let lines = render(&indicator);
        assert_eq!(lines.len(), 3);
        let text = plain_text(&lines);
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn spinner_animates_with_tick() {
        let mut a = ProgressIndicator::default();
        a.update(ProgressActivity { agent_busy: true, ..Default::default() });
        let mut b = ProgressIndicator::default();
        b.update(ProgressActivity { agent_busy: true, ..Default::default() });
        b.set_tick(1);
        let text_a = plain_text(&render(&a));
        let text_b = plain_text(&render(&b));
        assert_ne!(text_a, text_b);
    }

    #[test]
    fn on_tick_advances_when_busy() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        let tick_before = indicator.tick;
        indicator.on_tick();
        assert_ne!(indicator.tick, tick_before);
    }

    #[test]
    fn on_tick_noop_when_idle() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity::default());
        indicator.on_tick();
        assert!(render(&indicator).is_empty());
    }

    #[test]
    fn first_turn_shows_first_tip() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.set_turn_count(1);
        let text = plain_text(&render(&indicator));
        assert!(text.contains(MESSAGES[0]));
    }

    #[test]
    fn tip_advances_each_turn() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 1);
        let tip_0 = plain_text(&render(&indicator));

        indicator.update(ProgressActivity::default());

        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 2);
        let tip_1 = plain_text(&render(&indicator));

        assert_ne!(tip_0, tip_1);
        assert!(tip_0.contains(MESSAGES[0]));
        assert!(tip_1.contains(MESSAGES[1]));
    }

    #[test]
    fn shows_working_after_tips_exhausted() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.set_turn_count(MESSAGES.len() + 1);
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Working..."));
    }

    #[test]
    fn reset_restarts_tips() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 1);

        let indicator = ProgressIndicator::default();
        assert_eq!(indicator.turn_count, 0);
    }

    #[test]
    fn renders_workspace_move_without_interrupt_hint() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::Moving, ..Default::default() });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Moving workspace..."));
        assert!(!text.contains("esc to interrupt"));
    }

    #[test]
    fn workspace_move_message_takes_precedence_when_agent_is_busy() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity {
            agent_busy: true,
            workspace: WorkspaceProgress::Moving,
            ..Default::default()
        });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Moving workspace..."));
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn renders_workspace_session_load_without_interrupt_hint() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::LoadingSession, ..Default::default() });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Loading session in new workspace..."));
        assert!(!text.contains("esc to interrupt"));
    }

    #[test]
    fn non_agent_activity_does_not_advance_turn_tips() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::Moving, ..Default::default() });
        indicator.update(ProgressActivity::default());
        indicator.update(ProgressActivity { compaction_active: true, ..Default::default() });
        indicator.update(ProgressActivity::default());
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });

        let text = plain_text(&render(&indicator));
        assert!(text.contains(MESSAGES[0]));
    }

    #[test]
    fn staying_active_does_not_advance_tip() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 1);

        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 1);
    }

    #[test]
    fn compaction_active_shows_compacting_message() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { compaction_active: true, ..Default::default() });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Compacting context..."));
        assert!(!text.contains("esc to interrupt"));
    }

    #[test]
    fn compaction_active_with_agent_busy_shows_esc_hint() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, compaction_active: true, ..Default::default() });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Compacting context..."));
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn compaction_to_inactive_hides_indicator() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { compaction_active: true, ..Default::default() });
        assert!(!render(&indicator).is_empty());
        indicator.update(ProgressActivity::default());
        assert!(render(&indicator).is_empty());
    }

    #[test]
    fn workspace_move_failure_clears_indicator() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::Moving, ..Default::default() });
        assert!(!render(&indicator).is_empty());
        // workspace move failure: progress state goes back to None
        indicator.update(ProgressActivity::default());
        assert!(render(&indicator).is_empty());
    }

    #[test]
    fn session_loading_failure_clears_indicator() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::LoadingSession, ..Default::default() });
        assert!(!render(&indicator).is_empty());
        // session loading failure: progress state goes back to None
        indicator.update(ProgressActivity::default());
        assert!(render(&indicator).is_empty());
    }

    #[test]
    fn is_active_true_when_busy() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert!(indicator.is_active());
    }

    #[test]
    fn is_active_false_when_idle() {
        let indicator = ProgressIndicator::default();
        assert!(!indicator.is_active());
    }

    #[test]
    fn is_interruptible_true_for_agent_working() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert!(indicator.is_interruptible());
    }

    #[test]
    fn is_interruptible_false_for_non_agent_workspace_move() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::Moving, ..Default::default() });
        assert!(!indicator.is_interruptible());
    }

    #[test]
    fn reset_clears_all_state() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.on_tick();
        indicator.reset();
        assert!(!indicator.is_active());
        assert!(render(&indicator).is_empty());
        assert_eq!(indicator.turn_count, 0);
    }

    #[test]
    fn render_includes_padding() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        let lines = render(&indicator);
        let content_line = &lines[1];
        let first_span = &content_line.spans[0].content;
        assert_eq!(first_span, "  "); // padding of 2
    }

    #[test]
    fn precedence_workspace_over_compaction() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity {
            workspace: WorkspaceProgress::Moving,
            compaction_active: true,
            ..Default::default()
        });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Moving workspace..."));
        assert!(!text.contains("Compacting"));
    }

    #[test]
    fn precedence_compaction_over_agent_work() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, compaction_active: true, ..Default::default() });
        let text = plain_text(&render(&indicator));
        assert!(text.contains("Compacting context..."));
        assert!(!text.contains("Tip:"));
    }
}
