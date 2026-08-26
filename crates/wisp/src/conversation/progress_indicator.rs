use crate::theme::Theme;
use crate::view::wrap::tail_to_width;
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

#[derive(Debug)]
pub struct ProgressIndicator {
    phase: ProgressPhase,
    agent_phase: ProgressPhase,
    interruptible: bool,
    accepts_activity: bool,
    now: Instant,
    phase_started_at: Instant,
    thought: String,
}

impl Default for ProgressIndicator {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            phase: ProgressPhase::Idle,
            agent_phase: ProgressPhase::Idle,
            interruptible: false,
            accepts_activity: true,
            now,
            phase_started_at: now,
            thought: String::new(),
        }
    }
}

impl ProgressIndicator {
    pub(crate) fn accepts_activity(&self) -> bool {
        self.accepts_activity
    }

    pub(crate) fn prompt_started(&mut self) {
        self.thought.clear();
        self.accepts_activity = true;
        self.set_agent_phase(ProgressPhase::Thinking);
    }

    pub(crate) fn response_started(&mut self) {
        self.set_agent_phase(ProgressPhase::Responding);
    }

    pub(crate) fn tool_activity(&mut self) {
        self.set_agent_phase(ProgressPhase::Working);
    }

    pub(crate) fn prompt_finished(&mut self) {
        self.thought.clear();
        self.set_agent_phase(ProgressPhase::Idle);
        self.accepts_activity = false;
    }

    pub(crate) fn refresh(&mut self, override_phase: Option<ProgressPhase>, interruptible: bool) {
        let phase = override_phase.unwrap_or(self.agent_phase);
        if phase != self.phase {
            self.phase_started_at = self.now;
        }
        self.phase = phase;
        self.interruptible = interruptible;
    }

    pub(crate) fn record_thought(&mut self, chunk: &str) {
        if !self.accepts_activity {
            return;
        }
        self.set_agent_phase(ProgressPhase::Thinking);
        for character in chunk.chars() {
            if character.is_whitespace() {
                if !self.thought.is_empty() && !self.thought.ends_with(' ') {
                    self.thought.push(' ');
                }
            } else {
                self.thought.push(character);
            }
        }
        let excess = self.thought.chars().count().saturating_sub(THOUGHT_TAIL_CAPACITY);
        if excess > 0 {
            let cut = self.thought.char_indices().nth(excess).map_or(self.thought.len(), |(index, _)| index);
            self.thought.drain(..cut);
        }
    }

    pub(crate) fn on_tick(&mut self, now: Instant) {
        self.now = now;
    }

    fn set_agent_phase(&mut self, phase: ProgressPhase) {
        if phase != ProgressPhase::Idle && !self.accepts_activity {
            return;
        }
        if self.agent_phase == ProgressPhase::Thinking && phase != ProgressPhase::Thinking {
            self.thought.clear();
        }
        self.agent_phase = phase;
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

fn format_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    if seconds < 60 { format!("{seconds}s") } else { format!("{}m{:02}s", seconds / 60, seconds % 60) }
}

const THOUGHT_TAIL_CAPACITY: usize = 240;
const INTERRUPT_HINT: &str = "  (esc to interrupt)";
