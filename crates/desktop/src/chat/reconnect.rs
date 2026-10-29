use super::transcript::line;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::{Button, ButtonVariant, IconName, SurfaceExt, UiTheme};
use gpui::{Div, Entity, div, prelude::*};

pub(crate) fn reconnect_banner(thread: &Entity<Thread>, status: ThreadStatus, theme: &UiTheme) -> Option<Div> {
    let problem = status.problem()?;
    Some(
        div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .elevated(*theme)
            .child(line(IconName::TriangleAlert, theme.colors.warning, problem).flex_1().min_w_0())
            .child(reconnect_button(thread, theme)),
    )
}

pub(crate) fn reconnect_button(thread: &Entity<Thread>, theme: &UiTheme) -> Button {
    let thread = thread.clone();
    Button::new("transcript-reconnect", "Reconnect", *theme)
        .icon(IconName::RotateCw)
        .variant(ButtonVariant::Primary)
        .on_click(move |_, _, cx| thread.update(cx, Thread::reconnect))
}
