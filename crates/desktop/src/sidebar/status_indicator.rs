use crate::threads::ThreadStatus;
use crate::ui::tokens::{ICON_SIZE, RADIUS_CONTAINER, UNREAD_DOT_SIZE, rem};
use crate::ui::{Icon, IconName, Typography, TypographyExt, UiTheme};
use gpui::{AnyElement, Context, IntoElement, ParentElement, Render, Role, Window, div, prelude::*, svg};

/// Activity that needs an indicator in front of a thread's title. Unread content is separate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIndicator {
    Connecting,
    Running,
    NeedsInput,
    Errored,
    Disconnected,
}

impl StatusIndicator {
    /// Idle and stopped threads stay quiet, whether or not their last response is unread.
    pub fn of(status: &ThreadStatus) -> Option<Self> {
        match status {
            ThreadStatus::Connecting => Some(Self::Connecting),
            ThreadStatus::Running => Some(Self::Running),
            ThreadStatus::NeedsInput => Some(Self::NeedsInput),
            ThreadStatus::Errored(_) => Some(Self::Errored),
            ThreadStatus::Disconnected => Some(Self::Disconnected),
            ThreadStatus::Idle | ThreadStatus::Dormant => None,
        }
    }

    /// The activity's name in debug selectors.
    pub fn name(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Running => "running",
            Self::NeedsInput => "needs-input",
            Self::Errored => "errored",
            Self::Disconnected => "disconnected",
        }
    }

    /// Explains the activity both on hover and to assistive technology.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "Starting agent",
            Self::Running => "Agent working",
            Self::NeedsInput => "Waiting for your answer",
            Self::Errored => "Unable to connect — reconnect to try again",
            Self::Disconnected => "Disconnected — reconnect to continue",
        }
    }

    pub(crate) fn render(self, row: &str, theme: &UiTheme) -> AnyElement {
        let colors = theme.colors;
        let selector = format!("{row}-status-{}", self.name());
        let (shape, icon) = match self {
            Self::Connecting | Self::Running => (
                "spinner",
                Icon::new(IconName::LoaderCircle, colors.text_muted).spin(selector.clone()).into_any_element(),
            ),
            Self::NeedsInput => (
                "speech-bubble",
                svg()
                    .data(include_bytes!("../../assets/icons/message-circle.svg").as_slice())
                    .size(rem(ICON_SIZE))
                    .text_color(colors.warning)
                    .into_any_element(),
            ),
            Self::Errored | Self::Disconnected => (
                "warning-triangle",
                svg()
                    .data(include_bytes!("../../assets/icons/triangle-alert.svg").as_slice())
                    .size(rem(ICON_SIZE))
                    .text_color(colors.destructive)
                    .into_any_element(),
            ),
        };
        let shape_selector = format!("{selector}-{shape}");
        hint(
            selector,
            self.label(),
            div().flex().debug_selector(move || shape_selector).child(icon).into_any_element(),
            theme,
        )
    }
}

/// A trailing unread marker, independent of whether the agent is busy or needs attention.
pub(crate) fn render_unread(row: &str, theme: &UiTheme) -> AnyElement {
    hint(
        format!("{row}-unread"),
        "Unread response",
        div().size(rem(UNREAD_DOT_SIZE)).rounded_full().bg(theme.colors.accent).into_any_element(),
        theme,
    )
}

struct StatusTooltip {
    label: &'static str,
    theme: UiTheme,
}

impl Render for StatusTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let colors = self.theme.colors;
        div()
            .px_2()
            .py_1()
            .rounded(rem(RADIUS_CONTAINER))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .shadow_md()
            .typography(Typography::Small)
            .text_color(colors.text)
            .child(crate::ui::plain_text(self.label))
    }
}

fn hint(selector: String, label: &'static str, child: AnyElement, theme: &UiTheme) -> AnyElement {
    let theme = *theme;
    div()
        .id(selector.clone())
        .debug_selector(move || selector)
        .role(Role::Image)
        .aria_label(label)
        .flex_none()
        .flex()
        .items_center()
        .tooltip(move |_, cx| cx.new(|_| StatusTooltip { label, theme }).into())
        .child(child)
        .into_any_element()
}
