use super::Sidebar;
use super::listing::{Row, RowKind};
use crate::registry::Registry;
use crate::remotes::{InstanceState, Lifecycle};
use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadManager, ThreadStatus};
use crate::ui::tokens::{RADIUS_INNER, UNREAD_DOT_SIZE, rem};
use crate::ui::{
    Icon, IconButton, IconName, ListRow, MenuItem, StyledExt, Typography, TypographyExt, UiTheme, Waveform, hint,
    icon_slot, tagged, text_selector, with_menu,
};
use gpui::{AnyElement, App, ClickEvent, Context, Div, Entity, MouseButton, WeakEntity, Window, div, prelude::*};
use gpui_base::input::{Enter, Escape, Input, InputState};

pub(super) struct SidebarRow {
    sidebar: WeakEntity<Sidebar>,
    row: Row,
}

#[derive(Clone, PartialEq)]
pub(super) enum RowIcon {
    Status(ThreadStatus),
    Instance(Option<InstanceState>),
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum RowAction {
    Rename,
    Pin,
    Unpin,
    Archive,
    Stop,
    Suspend,
    Resume,
    StopInstance,
}

impl SidebarRow {
    pub(super) fn new(sidebar: WeakEntity<Sidebar>, row: Row) -> Self {
        Self { sidebar, row }
    }

    pub(super) fn show(&mut self, row: Row, cx: &mut Context<Self>) {
        if self.row != row {
            self.row = row;
            cx.notify();
        }
    }
}

impl Render for SidebarRow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let colors = theme.colors;
        let row = &self.row;
        let Row { icon, unread, selected, actions, kind, .. } = row;
        let editing = match kind {
            RowKind::Local { editing, .. } => editing.as_ref(),
            RowKind::Cloud { .. } => None,
        };
        let selector = row.key.selector();
        let hover_group = ListRow::hover_group(&selector);
        let hover_actions = actions.iter().filter(|action| action.hover()).map(|&action| {
            let (id, label, icon) = action.spec();
            let run = runner(&self.sidebar, row, action);
            IconButton::new(format!("{selector}-{id}"), icon, label, theme)
                .on_click(move |_, window, cx| run(window, cx))
        });
        let reconnect = row
            .live()
            .filter(|_| matches!(icon, RowIcon::Status(status) if status.problem().is_some()))
            .map(|thread| {
                let thread = thread.clone();
                IconButton::new(format!("{selector}-reconnect"), IconName::RotateCw, "Reconnect", theme)
                    .on_click(move |_, _, cx| thread.update(cx, Thread::reconnect))
            });
        let muted = matches!(icon, RowIcon::Status(ThreadStatus::Dormant) | RowIcon::Instance(_));
        let title = if let Some(input) = editing {
            editor(input, &self.sidebar, &theme)
        } else {
            let title_selector = format!("{selector}-title");
            div()
                .min_w_0()
                .truncate()
                .debug_selector(move || title_selector)
                .when(muted, |title| title.text_color(colors.text_muted))
                .child(row.title.clone())
                .into_any_element()
        };
        let unread = unread.then(|| {
            let dot = div().size(rem(UNREAD_DOT_SIZE)).rounded_full().bg(colors.accent);
            hint(format!("{selector}-unread"), "Unread response", dot, &theme)
        });
        let title = div()
            .when(editing.is_none(), |title| {
                title.debug_selector(|| text_selector(&format!("{selector}-title"), &row.title))
            })
            .min_w_0()
            .flex()
            .items_center()
            .gap_1()
            .child(title)
            .children(unread);
        let icon = match icon {
            RowIcon::Status(status) => status_icon(&selector, *status, &theme),
            RowIcon::Instance(state) => instance_icon(&selector, *state, &theme),
        };
        let two_line = matches!(kind, RowKind::Cloud { .. });
        let (body, trailing) = match kind {
            RowKind::Local { age, .. } => {
                (title.flex_1(), trailing(&selector, age.clone(), hover_actions, *selected, &theme).children(reconnect))
            }
            RowKind::Cloud { subtitle, expiring, .. } => {
                let subtitle = div()
                    .min_w_0()
                    .truncate()
                    .debug_selector(|| text_selector(&format!("{selector}-subtitle"), subtitle))
                    .typography(Typography::Caption)
                    .text_color(if *expiring { colors.warning } else { colors.text_muted })
                    .child(subtitle.clone());
                let actions = div().flex().items_center().reveal_on_hover(hover_group).children(hover_actions);
                (
                    div().flex_1().min_w_0().py_1().flex().flex_col().child(title).child(subtitle),
                    div().flex_none().flex().items_center().children(reconnect).child(actions),
                )
            }
        };
        let list_row = ListRow::new(selector.clone(), theme)
            .when(!two_line, |list_row| list_row.depth(1))
            .selected(*selected)
            .child(icon_slot())
            .child(icon_slot().when(two_line, |slot| slot.self_start().mt_2()).child(icon))
            .child(body)
            .child(trailing);
        if editing.is_some() {
            return div().w_full().child(list_row).into_any_element();
        }
        let (sidebar, opened) = (self.sidebar.clone(), row.clone());
        let list_row = list_row.on_click(move |_: &ClickEvent, _, cx| {
            let _ =
                sidebar.update(cx, |sidebar, cx| sidebar.threads.update(cx, |threads, cx| open(&opened, threads, cx)));
        });
        let (sidebar, row, actions) = (self.sidebar.clone(), row.clone(), actions.clone());
        let menu = move |_: &mut Window, _: &mut App| {
            let items = actions.into_iter().map(|action| {
                let (id, label, icon) = action.spec();
                MenuItem::new(format!("menu-{id}"), label, runner(&sidebar, &row, action)).icon(icon)
            });
            items.collect()
        };
        with_menu(&selector, list_row, MouseButton::Right, menu, &theme).w_full().into_any_element()
    }
}

impl RowAction {
    fn spec(self) -> (&'static str, &'static str, IconName) {
        match self {
            Self::Rename => ("rename", "Rename", IconName::Pencil),
            Self::Pin => ("pin", "Pin", IconName::Pin),
            Self::Unpin => ("unpin", "Unpin", IconName::PinOff),
            Self::Archive => ("archive", "Archive", IconName::Archive),
            Self::Stop => ("stop", "Stop thread", IconName::Square),
            Self::Suspend => ("suspend", "Suspend", IconName::Pause),
            Self::Resume => ("resume", "Resume", IconName::Play),
            Self::StopInstance => ("stop-instance", "Stop instance", IconName::Power),
        }
    }

    fn hover(self) -> bool {
        matches!(self, Self::Archive | Self::Stop | Self::StopInstance)
    }

    fn run(self, row: &Row, sidebar: &mut Sidebar, window: &mut Window, cx: &mut Context<Sidebar>) {
        let thread = row.live().cloned();
        let stop = |sidebar: &mut Sidebar, cx: &mut Context<Sidebar>| {
            if let Some(thread) = &thread {
                sidebar.threads.update(cx, |threads, cx| threads.stop(thread, cx));
            }
        };
        match (self, &row.kind) {
            (Self::Rename, RowKind::Local { session_id: Some(session_id), .. }) => {
                sidebar.start_rename(session_id.clone(), &row.title, window, cx);
            }
            (Self::Pin | Self::Unpin, RowKind::Local { session_id: Some(session_id), .. }) => {
                Registry::update(&sidebar.registry, cx, |state| state.toggle_pinned(session_id.clone()));
            }
            (Self::Archive, RowKind::Local { session_id: Some(session_id), .. }) => {
                Registry::update(&sidebar.registry, cx, |state| state.archive(session_id.clone()));
                stop(sidebar, cx);
            }
            (Self::Stop, _) => stop(sidebar, cx),
            (
                Self::Suspend | Self::Resume | Self::StopInstance,
                RowKind::Cloud { remote, instance_id: Some(id), .. },
            ) => {
                let change = match self {
                    Self::Suspend => Lifecycle::Suspend,
                    Self::Resume => Lifecycle::Resume,
                    _ => {
                        stop(sidebar, cx);
                        Lifecycle::Stop
                    }
                };
                sidebar.fleet.update(cx, |fleet, cx| fleet.change(&remote.name, id, change, cx));
            }
            _ => {}
        }
    }
}

fn runner(sidebar: &WeakEntity<Sidebar>, row: &Row, action: RowAction) -> impl Fn(&mut Window, &mut App) + 'static {
    let (sidebar, row) = (sidebar.clone(), row.clone());
    move |window, cx| {
        let _ = sidebar.update(cx, |sidebar, cx| action.run(&row, sidebar, window, cx));
    }
}

fn open(row: &Row, threads: &mut ThreadManager, cx: &mut Context<ThreadManager>) {
    match (&row.thread, &row.kind) {
        (Some(thread), _) => threads.open(thread, cx),
        (None, RowKind::Cloud { remote, instance_id: Some(id), .. }) => drop(threads.attach(remote, id, cx)),
        (None, _) => {}
    }
}

fn trailing(
    selector: &str,
    age: Option<String>,
    actions: impl IntoIterator<Item = IconButton>,
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
            let age_selector = format!("{selector}-age-{age}");
            div()
                .debug_selector(move || age_selector)
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
                .reveal_on_hover(hover_group)
                .children(actions),
        )
}

fn editor(input: &Entity<InputState>, sidebar: &WeakEntity<Sidebar>, theme: &UiTheme) -> AnyElement {
    let (commit, cancel) = (sidebar.clone(), sidebar.clone());
    div()
        .debug_selector(|| "rename-input".to_owned())
        .w_full()
        .min_w_0()
        .px_1()
        .rounded(rem(RADIUS_INNER))
        .border_1()
        .border_color(theme.colors.accent)
        .bg(theme.colors.canvas)
        .on_action(move |_: &Enter, _, cx| {
            let _ = commit.update(cx, Sidebar::commit_rename);
        })
        .on_action(move |_: &Escape, _, cx| {
            let _ = cancel.update(cx, Sidebar::cancel_rename);
        })
        .child(Input::new(input))
        .into_any_element()
}

fn status_icon(row: &str, status: ThreadStatus, theme: &UiTheme) -> AnyElement {
    let colors = theme.colors;
    let (name, label, shape, icon, color) = match status {
        ThreadStatus::Connecting => ("connecting", "Starting agent", "waveform", None, colors.text_muted),
        ThreadStatus::Running => ("running", "Agent working", "waveform", None, colors.text_muted),
        ThreadStatus::Background => {
            ("background", "Background task still running", "dot", Some(IconName::CircleDot), colors.text_muted)
        }
        ThreadStatus::NeedsInput => {
            ("needs-input", "Waiting for your answer", "speech-bubble", Some(IconName::MessageCircle), colors.warning)
        }
        ThreadStatus::Errored => (
            "errored",
            "Unable to connect — reconnect to try again",
            "warning-triangle",
            Some(IconName::TriangleAlert),
            colors.destructive,
        ),
        ThreadStatus::Disconnected => (
            "disconnected",
            "Disconnected — reconnect to continue",
            "warning-triangle",
            Some(IconName::TriangleAlert),
            colors.destructive,
        ),
        ThreadStatus::Idle | ThreadStatus::Dormant => {
            let icon = Icon::new(IconName::MessageSquareText, colors.text_muted);
            return tagged(format!("{row}-conversation-icon"), icon).into_any_element();
        }
    };
    let selector = format!("{row}-status-{name}");
    let icon = match icon {
        Some(icon) => Icon::new(icon, color).into_any_element(),
        None => Waveform::new(selector.clone(), color).into_any_element(),
    };
    hint(selector.clone(), label, tagged(format!("{selector}-{shape}"), icon), theme)
}

fn instance_icon(row: &str, state: Option<InstanceState>, theme: &UiTheme) -> AnyElement {
    let muted = theme.colors.text_muted;
    let (name, label, icon) = match state {
        Some(InstanceState::Suspended) => ("suspended", "Suspended", Some(IconName::Pause)),
        Some(InstanceState::Running) | None => ("unattached", "Running, with no thread on it", Some(IconName::Unplug)),
        Some(_) => ("settling", "Changing state", None),
    };
    let selector = format!("{row}-instance-{name}");
    let icon = match icon {
        Some(icon) => Icon::new(icon, muted).into_any_element(),
        None => Waveform::new(selector.clone(), muted).into_any_element(),
    };
    hint(selector.clone(), label, tagged(format!("{selector}-icon"), icon), theme)
}
