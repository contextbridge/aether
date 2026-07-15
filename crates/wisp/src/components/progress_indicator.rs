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
enum ProgressDisplay {
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

    #[cfg(test)]
    pub fn set_tick(&mut self, tick: u16) {
        self.tick = tick;
    }

    #[cfg(test)]
    pub fn set_turn_count(&mut self, count: usize) {
        self.turn_count = count;
    }

    fn is_active(&self) -> bool {
        self.display.is_active()
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
        let spinner_color = if matches!(self.display, ProgressDisplay::Compacting { .. }) {
            context.theme.warning()
        } else {
            context.theme.info()
        };
        line.push_styled(frame_char.to_string(), spinner_color);
        line.push_styled(format!(" {}", self.current_message()), context.theme.text_secondary());
        if self.display.is_interruptible() {
            line.push_with_style("  (esc to interrupt)".to_string(), Style::fg(context.theme.muted()).italic());
        }

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
    fn renders_nothing_after_busy_clears() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.update(ProgressActivity::default());
        assert!(indicator.render(&ctx()).lines().is_empty());
    }

    #[test]
    fn renders_esc_hint_when_agent_busy() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        let frame = indicator.render(&ctx());
        let lines = frame.lines();
        assert_eq!(lines.len(), 3);
        let text = lines[1].plain_text();
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn spinner_animates_with_tick() {
        let mut a = ProgressIndicator::default();
        a.update(ProgressActivity { agent_busy: true, ..Default::default() });
        let mut b = ProgressIndicator::default();
        b.update(ProgressActivity { agent_busy: true, ..Default::default() });
        b.set_tick(1);
        let text_a = a.render(&ctx()).lines()[1].plain_text();
        let text_b = b.render(&ctx()).lines()[1].plain_text();
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
        assert!(indicator.render(&ctx()).lines().is_empty());
    }

    #[test]
    fn first_turn_shows_first_tip() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.set_turn_count(1);
        let frame = indicator.render(&ctx());
        let text = frame.lines()[1].plain_text();
        assert!(text.contains(MESSAGES[0]));
    }

    #[test]
    fn tip_advances_each_turn() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 1);
        let tip_0 = indicator.render(&ctx()).lines()[1].plain_text();

        indicator.update(ProgressActivity::default());

        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        assert_eq!(indicator.turn_count, 2);
        let tip_1 = indicator.render(&ctx()).lines()[1].plain_text();

        assert_ne!(tip_0, tip_1);
        assert!(tip_0.contains(MESSAGES[0]));
        assert!(tip_1.contains(MESSAGES[1]));
    }

    #[test]
    fn shows_working_after_tips_exhausted() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() });
        indicator.set_turn_count(MESSAGES.len() + 1);
        let text = indicator.render(&ctx()).lines()[1].plain_text();
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
        let text = indicator.render(&ctx()).lines()[1].plain_text();
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
        let text = indicator.render(&ctx()).lines()[1].plain_text();
        assert!(text.contains("Moving workspace..."));
        assert!(text.contains("esc to interrupt"));
    }

    #[test]
    fn renders_workspace_session_load_without_interrupt_hint() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { workspace: WorkspaceProgress::LoadingSession, ..Default::default() });
        let text = indicator.render(&ctx()).lines()[1].plain_text();
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

        let text = indicator.render(&ctx()).lines()[1].plain_text();
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
}
