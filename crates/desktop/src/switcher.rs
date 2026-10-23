//! The thread switcher: a fuzzy search over every thread, live and saved, across projects.

use crate::theme::DesktopTheme;
use crate::threads::Thread;
use crate::ui::tokens::rem;
use crate::ui::{PickerItem, SearchPicker};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{AnyElement, App, Entity, MouseButton, MouseDownEvent, Window, div, prelude::*};
use std::path::PathBuf;

/// One thread the switcher offers.
#[derive(Debug, Clone)]
pub(crate) struct SwitcherEntry {
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) target: SwitchTarget,
}

/// The thread a [`SwitcherEntry`] switches to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwitchTarget {
    Live(Entity<Thread>),
    /// A saved session no live thread has open, resumed in `cwd`.
    Saved {
        session_id: SessionId,
        cwd: PathBuf,
    },
}

/// A picker over `entries`, in their order until the user types.
pub(crate) fn picker(entries: &[SwitcherEntry], window: &mut Window, cx: &mut App) -> Entity<SearchPicker> {
    let items = entries
        .iter()
        .map(|entry| {
            let id = match &entry.target {
                SwitchTarget::Live(thread) => format!("thread-{}", thread.read(cx).number()),
                SwitchTarget::Saved { session_id, .. } => format!("session-{}", session_id.0),
            };
            PickerItem::new(id, entry.title.clone()).detail(entry.project.clone())
        })
        .collect();
    let theme = DesktopTheme::global(cx).ui();
    cx.new(|cx| {
        SearchPicker::new("switcher", items, theme, cx)
            .searchable("Search threads…", window, cx)
            .empty("No matching threads")
            .footer("↑↓ Move    ↵ Open    Esc Close")
            .width(rem(SWITCHER_WIDTH))
            .height(rem(SWITCHER_HEIGHT))
    })
}

/// Shows `picker` over a scrim that dismisses it when clicked. It is tagged `thread-switcher`.
pub(crate) fn overlay(
    picker: Entity<SearchPicker>,
    cx: &App,
    on_dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id("thread-switcher")
        .debug_selector(|| "thread-switcher".to_owned())
        .absolute()
        .inset_0()
        .flex()
        .flex_col()
        .items_center()
        .pt(rem(SWITCHER_TOP))
        .px_4()
        .bg(DesktopTheme::global(cx).ui().colors.scrim)
        .occlude()
        .on_mouse_down(MouseButton::Left, on_dismiss)
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(picker))
        .into_any_element()
}

const SWITCHER_WIDTH: f32 = 560.0;
const SWITCHER_HEIGHT: f32 = 416.0;
/// How far below the window's top the switcher opens, so it reads as a palette over the content.
const SWITCHER_TOP: f32 = 96.0;
