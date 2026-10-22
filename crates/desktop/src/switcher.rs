//! The thread switcher: a fuzzy search over every thread, live and saved, across projects.

use crate::fuzzy;
use crate::theme::DesktopTheme;
use crate::threads::ThreadId;
use crate::ui::tokens::{BAR_HEIGHT, GROUP_HEIGHT, RADIUS_CONTAINER, rem};
use crate::ui::{Icon, IconName, ListRow, Typography, TypographyExt};
use agent_client_protocol::schema::v2::SessionId;
use gpui::{AnyElement, ClickEvent, Context, Entity, EventEmitter, MouseButton, Subscription, Window, div, prelude::*};
use gpui_base::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use std::path::PathBuf;

/// Lists the threads matching what the user types, best match first, with one highlighted.
///
/// ↑ and ↓ move the highlight, Enter or a click chooses a thread, and Escape dismisses the
/// switcher. What to do with the choice is up to whoever listens for [`SwitcherEvent`]s.
pub(crate) struct ThreadSwitcher {
    input: Entity<InputState>,
    entries: Vec<SwitcherEntry>,
    /// Indices into `entries` of those matching the query, best match first.
    matches: Vec<usize>,
    /// The position in `matches` of the highlighted entry.
    highlighted: usize,
    _subscription: Subscription,
}

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
    Live(ThreadId),
    /// A saved session no live thread has open, resumed in `cwd`.
    Saved {
        session_id: SessionId,
        cwd: PathBuf,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SwitcherEvent {
    Chose(SwitchTarget),
    Dismissed,
}

impl ThreadSwitcher {
    /// Offers `entries`, in their order until the user types, with the search field focused.
    pub(crate) fn new(entries: Vec<SwitcherEntry>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let style = DesktopTheme::global(cx).editor_style();
        let input = cx.new(|cx| {
            let mut input = InputState::new(window, cx).placeholder("Search threads…");
            input.set_editor_style(style);
            input
        });
        input.update(cx, |input, cx| input.focus(window, cx));
        let subscription = cx.subscribe(&input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.refilter(cx);
            }
        });
        let matches = (0..entries.len()).collect();
        Self { input, entries, matches, highlighted: 0, _subscription: subscription }
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value();
        self.matches = if query.trim().is_empty() {
            (0..self.entries.len()).collect()
        } else {
            let keys = self.entries.iter().map(|entry| format!("{} {}", entry.title, entry.project));
            fuzzy::filter(&query, keys)
        };
        self.highlighted = 0;
        cx.notify();
    }

    /// Moves the highlight `by` entries, stopping at either end of the list.
    fn step(&mut self, by: isize, cx: &mut Context<Self>) {
        let last = self.matches.len().saturating_sub(1);
        self.highlighted = self.highlighted.saturating_add_signed(by).min(last);
        cx.notify();
    }

    fn choose(&mut self, position: usize, cx: &mut Context<Self>) {
        if let Some(entry) = self.matches.get(position).map(|&index| &self.entries[index]) {
            cx.emit(SwitcherEvent::Chose(entry.target.clone()));
        }
    }

    fn render_entry(&self, position: usize, entry: &SwitcherEntry, cx: &mut Context<Self>) -> AnyElement {
        let theme = DesktopTheme::global(cx).ui();
        let selector = match &entry.target {
            SwitchTarget::Live(id) => format!("switcher-thread-{id}"),
            SwitchTarget::Saved { session_id, .. } => format!("switcher-session-{}", session_id.0),
        };
        ListRow::new(selector, theme)
            .selected(position == self.highlighted)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(position, cx)))
            .child(div().flex_1().min_w_0().truncate().child(entry.title.clone()))
            .child(
                div()
                    .flex_none()
                    .typography(Typography::Small)
                    .text_color(theme.colors.text_muted)
                    .child(entry.project.clone()),
            )
            .into_any_element()
    }
}

impl EventEmitter<SwitcherEvent> for ThreadSwitcher {}

impl Render for ThreadSwitcher {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let rows: Vec<AnyElement> = self
            .matches
            .iter()
            .enumerate()
            .map(|(position, &index)| self.render_entry(position, &self.entries[index], cx))
            .collect();
        let colors = theme.colors;
        let search = div()
            .flex_none()
            .h(rem(BAR_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(colors.border)
            .child(Icon::new(IconName::Search, colors.text_muted))
            .child(div().flex_1().min_w_0().child(Input::new(&self.input)));
        let list = if rows.is_empty() {
            div().flex_1().p_4().typography(Typography::Body).text_color(colors.text_muted).child("No matching threads")
        } else {
            div()
                .flex_1()
                .min_h_0()
                .child(div().id("thread-switcher-list").size_full().overflow_y_scroll().p_1().children(rows))
        };
        let hints = div()
            .flex_none()
            .h(rem(GROUP_HEIGHT))
            .px_3()
            .flex()
            .items_center()
            .gap_4()
            .border_t_1()
            .border_color(colors.border)
            .typography(Typography::Caption)
            .text_color(colors.text_muted)
            .child("↑↓ Move")
            .child("↵ Open")
            .child("Esc Close");
        let panel = div()
            .debug_selector(|| "thread-switcher".to_owned())
            .w(rem(SWITCHER_WIDTH))
            .max_w_full()
            .h(rem(SWITCHER_HEIGHT))
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(rem(RADIUS_CONTAINER))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .shadow_lg()
            // Captured on the way down to the search field, which would otherwise move its cursor.
            .capture_action(cx.listener(|this, _: &MoveUp, _, cx| {
                this.step(-1, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|this, _: &MoveDown, _, cx| {
                this.step(1, cx);
                cx.stop_propagation();
            }))
            .on_action(cx.listener(|this, _: &Enter, _, cx| this.choose(this.highlighted, cx)))
            .on_action(cx.listener(|_, _: &Escape, _, cx| cx.emit(SwitcherEvent::Dismissed)))
            .child(search)
            .child(list)
            .child(hints);
        div()
            .id("thread-switcher-backdrop")
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .pt(rem(SWITCHER_TOP))
            .px_4()
            .bg(colors.scrim)
            .occlude()
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(SwitcherEvent::Dismissed)))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(div().on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(panel))
    }
}

const SWITCHER_WIDTH: f32 = 560.0;
const SWITCHER_HEIGHT: f32 = 416.0;
/// How far below the window's top the switcher opens, so it reads as a palette over the content.
const SWITCHER_TOP: f32 = 96.0;
