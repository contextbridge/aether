use crate::theme::Theme;
use crate::view::wrap::tail_to_width;
use acp_utils::conversation::{Activity, ActivityPhase};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use std::time::{Duration, Instant};
use unicode_width::UnicodeWidthStr;

pub const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn spinner_frame(tick: usize) -> &'static str {
    SPINNER_FRAMES[tick % SPINNER_FRAMES.len()]
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum ProgressPhase {
    #[default]
    Idle,
    Thinking,
    Responding,
    RequiresAction,
    Working,
    Compacting,
    MovingWorkspace,
    LoadingSession,
}

impl ProgressPhase {
    fn label(self) -> &'static str {
        match self {
            Self::Idle => "",
            Self::Thinking => "Thinking…",
            Self::Responding => "Responding…",
            Self::RequiresAction => "Waiting for action…",
            Self::Working => "Working…",
            Self::Compacting => "Compacting context...",
            Self::MovingWorkspace => "Moving workspace...",
            Self::LoadingSession => "Loading session in new workspace...",
        }
    }

    fn spinner_color(self, theme: &Theme) -> Color {
        if self == Self::Compacting { theme.warning } else { theme.info }
    }
}

impl From<ActivityPhase> for ProgressPhase {
    fn from(phase: ActivityPhase) -> Self {
        match phase {
            ActivityPhase::Idle => Self::Idle,
            ActivityPhase::Thinking => Self::Thinking,
            ActivityPhase::Responding => Self::Responding,
            ActivityPhase::RequiresAction => Self::RequiresAction,
            ActivityPhase::Working => Self::Working,
        }
    }
}

#[derive(Debug)]
pub struct ProgressIndicator {
    phase: ProgressPhase,
    interruptible: bool,
    now: Instant,
    phase_started_at: Instant,
    thought: String,
}

impl Default for ProgressIndicator {
    fn default() -> Self {
        let now = Instant::now();
        Self { phase: ProgressPhase::Idle, interruptible: false, now, phase_started_at: now, thought: String::new() }
    }
}

impl ProgressIndicator {
    /// Show the agent's activity, unless the host's own operation overrides it.
    pub(crate) fn refresh(&mut self, activity: &Activity, override_phase: Option<ProgressPhase>, interruptible: bool) {
        let phase = override_phase.unwrap_or_else(|| activity.phase().into());
        if phase != self.phase {
            self.phase_started_at = self.now;
        }
        self.phase = phase;
        self.interruptible = interruptible;
        self.thought = collapsed_tail(activity.thought(), THOUGHT_TAIL_CAPACITY);
    }

    pub(crate) fn on_tick(&mut self, now: Instant) {
        self.now = now;
    }

    pub fn is_active(&self) -> bool {
        self.phase != ProgressPhase::Idle
    }

    pub(crate) fn is_interruptible(&self) -> bool {
        self.interruptible && self.is_active()
    }

    pub(crate) fn height(&self) -> u16 {
        if self.is_active() { 3 } else { 0 }
    }

    fn lines(&self, theme: &Theme, tick: usize, width: u16) -> Vec<Line<'static>> {
        if !self.is_active() {
            return Vec::new();
        }
        vec![Line::default(), self.activity_line(theme, tick, width), Line::default()]
    }

    fn activity_line(&self, theme: &Theme, tick: usize, width: u16) -> Line<'static> {
        let label = format!(" {}", self.phase.label());
        let elapsed = format!("  {}", format_elapsed(self.now.saturating_duration_since(self.phase_started_at)));
        let hint = self.is_interruptible().then_some(INTERRUPT_HINT);
        let fixed = 1 + label.width() + elapsed.width() + hint.map_or(0, UnicodeWidthStr::width) + 1;
        let room = usize::from(width).saturating_sub(fixed);
        let mut spans = vec![
            Span::styled(spinner_frame(tick).to_string(), Style::new().fg(self.phase.spinner_color(theme))),
            Span::styled(label, Style::new().fg(theme.text_secondary)),
        ];
        if self.has_thought() && room > 0 {
            spans.push(Span::styled(
                format!(" {}", tail_to_width(&self.thought, room)),
                Style::new().fg(theme.blockquote).add_modifier(Modifier::ITALIC | Modifier::DIM),
            ));
        }
        spans.push(Span::styled(elapsed, Style::new().fg(theme.text_secondary)));
        if let Some(hint) = hint {
            spans.push(Span::styled(hint.to_string(), Style::new().fg(theme.muted).add_modifier(Modifier::ITALIC)));
        }
        Line::from(spans)
    }

    fn has_thought(&self) -> bool {
        self.phase == ProgressPhase::Thinking && !self.thought.is_empty()
    }
}

pub struct ProgressIndicatorView<'a> {
    indicator: &'a ProgressIndicator,
    theme: &'a Theme,
    tick: usize,
}

impl<'a> ProgressIndicatorView<'a> {
    pub fn new(indicator: &'a ProgressIndicator, theme: &'a Theme, tick: usize) -> Self {
        Self { indicator, theme, tick }
    }
}

impl Widget for ProgressIndicatorView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let height = usize::from(area.height);
        if height == 0 {
            return;
        }
        let mut lines = self.indicator.lines(self.theme, self.tick, area.width);
        if lines.len() > height {
            lines.pop();
        }
        if lines.len() > height {
            lines.remove(0);
        }
        lines.truncate(height);
        Paragraph::new(lines).render(area, buf);
    }
}

/// The last `capacity` characters of `text` with every whitespace run shown as
/// one space, read from the end so a long thought costs no more than its tail.
fn collapsed_tail(text: &str, capacity: usize) -> String {
    let mut reversed = Vec::with_capacity(capacity);
    let mut pending_space = false;
    for character in text.chars().rev() {
        if reversed.len() >= capacity {
            break;
        }
        if character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space {
            reversed.push(' ');
            pending_space = false;
        }
        reversed.push(character);
    }
    reversed.truncate(capacity);
    reversed.into_iter().rev().collect()
}

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m{:02}s", seconds / 60, seconds % 60) }
}

const THOUGHT_TAIL_CAPACITY: usize = 240;
const INTERRUPT_HINT: &str = "  (esc to interrupt)";
