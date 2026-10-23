use crate::threads::ThreadStatus;
use crate::ui::tokens::{ICON_SIZE, UNREAD_DOT_SIZE, rem};
use crate::ui::{Icon, IconButton, IconName, ListRow, Tooltip, Typography, TypographyExt, UiTheme};
use chrono::{DateTime, Utc};
use gpui::{AnyElement, Div, IntoElement, ParentElement, Role, div, prelude::*};

/// Formats how long ago `then` was, as of `now`, in the sidebar's compact style: `now`, `5m`,
/// `3h`, `2d`, `6w`, `1y`.
pub fn relative_time(then: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let elapsed = now.signed_duration_since(then);
    match (elapsed.num_minutes(), elapsed.num_hours(), elapsed.num_days()) {
        (..1, _, _) => "now".to_owned(),
        (minutes, ..1, _) => format!("{minutes}m"),
        (_, hours, ..1) => format!("{hours}h"),
        (_, _, days @ ..7) => format!("{days}d"),
        (_, _, days @ ..365) => format!("{}w", days / 7),
        (_, _, days) => format!("{}y", days / 365),
    }
}

/// One thread in a project section: its activity icon, or a conversation icon while it is quiet,
/// its `title` with an unread dot after it, and `trailing`.
pub(super) fn thread_row(
    selector: &str,
    title: AnyElement,
    status: &ThreadStatus,
    unread: bool,
    trailing: Div,
    theme: &UiTheme,
) -> ListRow {
    let unread = unread.then(|| {
        let dot = div().size(rem(UNREAD_DOT_SIZE)).rounded_full().bg(theme.colors.accent);
        hint(format!("{selector}-unread"), "Unread response", dot, theme)
    });
    ListRow::new(selector.to_owned(), *theme)
        .child(slot())
        .child(slot().child(status_icon(selector, status, theme)))
        .child(div().flex_1().min_w_0().flex().items_center().gap_1().child(title).children(unread))
        .child(trailing)
}

/// The thread's `age`, which `actions` cover while the pointer is over the row.
pub(super) fn trailing(
    selector: &str,
    age: Option<String>,
    actions: Vec<IconButton>,
    selected: bool,
    theme: &UiTheme,
) -> Div {
    let colors = theme.colors;
    let hover_group = ListRow::hover_group(selector);
    div()
        .flex_none()
        .relative()
        .flex()
        .items_center()
        .justify_end()
        .children(age.map(|age| {
            div()
                .typography(Typography::Caption)
                .text_color(colors.text_muted)
                .group_hover(hover_group.clone(), |age| age.opacity(0.0))
                .child(age)
        }))
        .child(
            div()
                .absolute()
                .right_0()
                .pl_1()
                .flex()
                .items_center()
                .bg(if selected { colors.surface_selected } else { colors.surface_hover })
                .opacity(0.0)
                .group_hover(hover_group, |actions| actions.opacity(1.0))
                .children(actions),
        )
}

pub(super) fn tagged(selector: String, child: impl IntoElement) -> Div {
    div().flex().debug_selector(move || selector).child(child)
}

pub(super) fn slot() -> Div {
    div().flex_none().size(rem(ICON_SIZE)).flex().items_center().justify_center()
}

fn status_icon(row: &str, status: &ThreadStatus, theme: &UiTheme) -> AnyElement {
    let colors = theme.colors;
    let (name, label, shape, icon, color) = match status {
        ThreadStatus::Connecting => {
            ("connecting", "Starting agent", "spinner", IconName::LoaderCircle, colors.text_muted)
        }
        ThreadStatus::Running => ("running", "Agent working", "spinner", IconName::LoaderCircle, colors.text_muted),
        ThreadStatus::NeedsInput => {
            ("needs-input", "Waiting for your answer", "speech-bubble", IconName::MessageCircle, colors.warning)
        }
        ThreadStatus::Errored(_) => (
            "errored",
            "Unable to connect — reconnect to try again",
            "warning-triangle",
            IconName::TriangleAlert,
            colors.destructive,
        ),
        ThreadStatus::Disconnected => (
            "disconnected",
            "Disconnected — reconnect to continue",
            "warning-triangle",
            IconName::TriangleAlert,
            colors.destructive,
        ),
        // Idle and stopped threads stay quiet, whether or not their last response is unread.
        ThreadStatus::Idle | ThreadStatus::Dormant => {
            let icon = Icon::new(IconName::MessageSquareText, colors.text_muted);
            return tagged(format!("{row}-conversation-icon"), icon).into_any_element();
        }
    };
    let selector = format!("{row}-status-{name}");
    let icon = Icon::new(icon, color).when(shape == "spinner", |icon| icon.spin(selector.clone()));
    hint(selector.clone(), label, tagged(format!("{selector}-{shape}"), icon), theme)
}

/// Explains `child` both on hover and to assistive technology.
fn hint(selector: String, label: &'static str, child: impl IntoElement, theme: &UiTheme) -> AnyElement {
    div()
        .id(selector.clone())
        .debug_selector(move || selector)
        .role(Role::Image)
        .aria_label(label)
        .flex_none()
        .flex()
        .items_center()
        .tooltip(Tooltip::text(label, *theme))
        .child(child)
        .into_any_element()
}
