use crate::components::theme::Theme;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

const MESSAGES: &[&str] = &[
    "Tip: Hit Tab to adjust reasoning level (off → low → medium → high)",
    "Tip: Hit Shift+Tab to cycle through modes",
    "Tip: Press @ to attach files to your prompt",
    "Tip: Type / to open the command picker",
    "Tip: Use /resume to pick up a previous session",
    "Tip: wisp-next supports custom themes — drop a .tmTheme in ~/.wisp/themes/",
    "Tip: Open /settings to change your model, theme, or view MCP server status",
    "Tip: The context gauge in the status bar shows current context usage against the model limit",
];

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
    turn_count: usize,
}

/// A one-frame rendering command for a persistent progress indicator.
pub struct ProgressIndicatorView<'a> {
    indicator: &'a ProgressIndicator,
    theme: &'a Theme,
    tick: usize,
}

impl<'a> ProgressIndicatorView<'a> {
    pub fn new(indicator: &'a ProgressIndicator, theme: &'a Theme, tick: usize) -> Self {
        Self { indicator, theme, tick }
    }

    pub fn line_count(&self) -> usize {
        self.indicator.is_active().then_some(3).unwrap_or(0)
    }

    pub fn height(&self) -> u16 {
        u16::try_from(self.line_count()).unwrap_or(u16::MAX)
    }
}

impl Widget for ProgressIndicatorView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(self.indicator.lines(self.theme, self.tick)).render(area, buf);
    }
}

impl ProgressIndicator {
    pub fn update(&mut self, activity: ProgressActivity, turn_count: usize) {
        self.display = activity.display();
        self.turn_count = turn_count;
    }

    pub fn is_active(&self) -> bool {
        self.display.is_active()
    }

    pub fn is_interruptible(&self) -> bool {
        self.display.is_interruptible()
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn lines(&self, theme: &Theme, tick: usize) -> Vec<Line<'static>> {
        if !self.is_active() {
            return Vec::new();
        }

        let spinner_color =
            if matches!(self.display, ProgressDisplay::Compacting { .. }) { theme.warning } else { theme.info };
        let mut spans = vec![
            Span::styled(spinner_frame(tick).to_string(), Style::new().fg(spinner_color)),
            Span::styled(format!(" {}", self.current_message()), Style::new().fg(theme.text_secondary)),
        ];
        if self.display.is_interruptible() {
            spans.push(Span::styled(
                "  (esc to interrupt)".to_string(),
                Style::new().fg(theme.muted).add_modifier(Modifier::ITALIC),
            ));
        }
        vec![Line::default(), Line::from(spans), Line::default()]
    }

    fn current_message(&self) -> &'static str {
        match self.display {
            ProgressDisplay::MovingWorkspace { .. } => "Moving workspace...",
            ProgressDisplay::LoadingSession { .. } => "Loading session in new workspace...",
            ProgressDisplay::Compacting { .. } => "Compacting context...",
            // Tips cycle so they keep rotating past the length of the list.
            ProgressDisplay::AgentWorking => match self.turn_count.checked_sub(1) {
                Some(turn) => MESSAGES[turn % MESSAGES.len()],
                None => "Working...",
            },
            ProgressDisplay::Idle => "",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_precedence_and_interruptibility_are_derived_from_state() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(
            ProgressActivity { agent_busy: true, workspace: WorkspaceProgress::Moving, compaction_active: true },
            1,
        );
        let theme = Theme::default();
        let text = indicator.lines(&theme, 0);
        assert!(text[1].to_string().contains("Moving workspace..."));
        assert!(indicator.is_interruptible());
    }

    #[test]
    fn tick_selects_the_shared_spinner_frame() {
        let mut indicator = ProgressIndicator::default();
        indicator.update(ProgressActivity { agent_busy: true, ..Default::default() }, 1);
        let theme = Theme::default();
        let first = indicator.lines(&theme, 0);
        let second = indicator.lines(&theme, 1);
        assert_ne!(first[1].to_string(), second[1].to_string());
    }
}
