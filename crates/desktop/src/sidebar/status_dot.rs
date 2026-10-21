use crate::threads::ThreadStatus;
use clankerdiff_gpui::ui::prelude::UiTheme;
use gpui::{
    Animation, AnimationExt, AnyElement, IntoElement, ParentElement, Styled, div, prelude::*, pulsating_between, px,
};
use std::time::Duration;

/// The dot in front of a thread row that tells what the thread needs from the user, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusDot {
    /// The agent is starting or working. Pulses.
    Running,
    /// A turn finished while the user was not watching the thread.
    Unread,
    /// The agent waits for the user to answer it.
    NeedsInput,
    /// The agent could not be reached, or its connection closed.
    Errored,
    /// No agent runs for the thread. Hollow.
    Dormant,
}

/// Whether the user has seen the turn a thread last finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    Read,
    /// The turn finished while the user was not watching the thread.
    Unread,
}

impl StatusDot {
    /// The dot for a thread in `status` whose last turn the user has `seen` or not. An idle
    /// thread with nothing unseen shows none.
    pub fn of(status: &ThreadStatus, seen: Seen) -> Option<Self> {
        match status {
            ThreadStatus::Connecting | ThreadStatus::Running => Some(Self::Running),
            ThreadStatus::NeedsInput => Some(Self::NeedsInput),
            ThreadStatus::Errored(_) | ThreadStatus::Disconnected => Some(Self::Errored),
            ThreadStatus::Dormant => Some(Self::Dormant),
            ThreadStatus::Idle => match seen {
                Seen::Unread => Some(Self::Unread),
                Seen::Read => None,
            },
        }
    }

    /// The dot's name in debug selectors, as in `thread-row-0-status-running`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Unread => "unread",
            Self::NeedsInput => "needs-input",
            Self::Errored => "errored",
            Self::Dormant => "dormant",
        }
    }

    /// Paints the dot for the row tagged `row`, tagging it `<row>-status-<name>`.
    pub(crate) fn render(self, row: &str, theme: &UiTheme) -> AnyElement {
        let colors = theme.colors;
        let selector = format!("{row}-status-{}", self.name());
        let dot = div().size(px(DOT_SIZE)).rounded_full();
        let dot = match self {
            Self::Running => dot
                .bg(colors.positive)
                .with_animation(
                    selector.clone(),
                    Animation::new(PULSE_PERIOD).repeat_synced().with_easing(pulsating_between(0.3, 1.0)),
                    Styled::opacity,
                )
                .into_any_element(),
            Self::Unread => dot.bg(colors.accent).into_any_element(),
            Self::NeedsInput => dot.bg(colors.warning).into_any_element(),
            Self::Errored => dot.bg(colors.destructive).into_any_element(),
            Self::Dormant => dot.border_1().border_color(colors.text_muted).into_any_element(),
        };
        div().debug_selector(move || selector).child(dot).into_any_element()
    }
}

const DOT_SIZE: f32 = 8.0;

const PULSE_PERIOD: Duration = Duration::from_millis(1600);
