//! The thread switcher: a fuzzy search over every thread, live and saved, across projects.

use crate::fuzzy;
use crate::theme::DesktopTheme;
use crate::threads::ThreadId;
use agent_client_protocol::schema::v2::SessionId;
use clankerdiff_gpui::ui::prelude::{ListRow, Modal, MutedText, SelectionState};
use gpui::{
    AnyElement, ClickEvent, Context, Entity, EventEmitter, SharedString, Subscription, Window, div, prelude::*,
};
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
        let state = if position == self.highlighted { SelectionState::Selected } else { SelectionState::None };
        let row = ListRow::new(SharedString::from(selector.clone()), theme)
            .state(state)
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.choose(position, cx)))
            .child(div().flex_1().min_w_0().truncate().child(entry.title.clone()))
            .child(div().flex_none().text_sm().child(MutedText::new(entry.project.clone(), theme)));
        div().debug_selector(move || selector).child(row).into_any_element()
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
        let search = div()
            .flex_none()
            .mb_2()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.colors.accent)
            .bg(theme.colors.canvas)
            .child(Input::new(&self.input));
        let content = div()
            .debug_selector(|| "thread-switcher".to_owned())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
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
            .child(div().id("thread-switcher-list").min_h_0().flex_1().overflow_y_scroll().children(rows));
        Modal::new("thread-switcher-modal", "Switch thread", theme)
            .hint("↑↓ to move · Enter to open · Esc to close")
            .width(SWITCHER_WIDTH)
            .height(SWITCHER_HEIGHT)
            .child(content)
    }
}

const SWITCHER_WIDTH: f32 = 560.0;
const SWITCHER_HEIGHT: f32 = 420.0;
