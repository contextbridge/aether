use crate::theme::DesktopTheme;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::tokens::{ICON_SIZE, ICON_SIZE_SMALL, ROW_HEIGHT, UNREAD_DOT_SIZE, rem};
use crate::ui::{
    Icon, IconName, SurfaceExt, Tooltip, Typography, TypographyExt, UiTheme, Waveform, text_selector, with_middots,
};
use acp_utils::conversation::{Activity, Conversation, is_plan_entry_finished};
use agent_client_protocol::schema::v2 as acp;
use client_core::format::token_count;
use client_core::session_core::SessionCore;
use gpui::{
    Anchor, AnyElement, App, ClickEvent, Context, Div, Entity, FontFeatures, Hsla, PathBuilder, Role, Stateful,
    Subscription, Window, anchored, canvas, deferred, div, point, prelude::*, px, relative,
};
use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) struct StatusLine {
    thread: Entity<Thread>,
    running_since: Option<Instant>,
    last_turn: Option<Duration>,
    plan_open: bool,
    _subscription: Subscription,
}

impl StatusLine {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&thread, |this, _, cx| {
            this.sync(cx);
            cx.notify();
        });
        let mut status =
            Self { thread, running_since: None, last_turn: None, plan_open: false, _subscription: subscription };
        status.sync(cx);
        status
    }

    fn sync(&mut self, cx: &App) {
        let running = matches!(self.thread.read(cx).status(), ThreadStatus::Running | ThreadStatus::NeedsInput);
        if !running {
            if let Some(since) = self.running_since.take() {
                self.last_turn = Some(since.elapsed());
            }
        } else if self.running_since.is_none() {
            self.running_since = Some(Instant::now());
        }
    }

    fn doing(&self, cx: &App) -> Option<Doing> {
        self.running_since?;
        let thread = self.thread.read(cx);
        if thread.status() == ThreadStatus::NeedsInput {
            return Some(Doing::Waiting);
        }
        Some(thread.core().map(SessionCore::conversation).map_or(Doing::Working, Doing::of))
    }

    fn toggle_plan(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.plan_open = !self.plan_open;
        cx.notify();
    }
}

impl Render for StatusLine {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let colors = theme.colors;
        let conversation = self.thread.read(cx).core().map(SessionCore::conversation);
        let plan = conversation.and_then(visible_plan);
        let usage = conversation.and_then(Conversation::context_usage);
        let doing = self.doing(cx);
        let waiting = matches!(doing, Some(Doing::Waiting));
        let label = doing.as_ref().map(|doing| doing.label(plan));
        let last_turn = self.last_turn.filter(|_| doing.is_none()).map(|took| {
            div()
                .debug_selector(|| "last-turn".to_owned())
                .text_color(colors.text_muted)
                .font_features(tabular_figures())
                .child(format!("Took {}", clock(took)))
        });
        let activity = (doing.is_some() || last_turn.is_some()).then(|| {
            div()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .children(doing.as_ref().map(|doing| indicator(doing, &theme)))
                .child(
                    div()
                        .debug_selector(|| text_selector("status-label", label.as_deref().unwrap_or_default()))
                        .min_w_0()
                        .truncate()
                        .when(waiting, |label| label.text_color(colors.warning))
                        .children(label)
                        .children(last_turn),
                )
                .into_any_element()
        });
        let parts = [
            activity,
            plan.map(|entries| plan_toggle(entries, self.plan_open, &theme, cx.listener(Self::toggle_plan))),
            self.running_since.map(|since| elapsed(since.elapsed(), &theme)),
            usage.map(|usage| context_meter(usage, &theme)),
        ];
        div()
            .debug_selector(|| "status".to_owned())
            .flex_1()
            .min_w_0()
            .h(rem(ROW_HEIGHT))
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .typography(Typography::Body)
            .text_color(colors.text_secondary)
            .children(with_middots(parts.into_iter().flatten(), &theme))
    }
}

enum Doing {
    Compacting,
    Thinking,
    Writing,
    Waiting,
    Working,
}

impl Doing {
    fn of(conversation: &Conversation) -> Self {
        match conversation.activity() {
            Activity::Compacting => Self::Compacting,
            Activity::Thinking => Self::Thinking,
            Activity::Responding => Self::Writing,
            Activity::RequiresAction => Self::Waiting,
            Activity::Working | Activity::Idle => Self::Working,
        }
    }

    fn label(&self, plan: Option<&[acp::PlanEntry]>) -> String {
        let step =
            plan.and_then(|entries| entries.iter().find(|entry| entry.status == acp::PlanEntryStatus::InProgress));
        match (self, step) {
            (Self::Waiting | Self::Compacting, _) | (_, None) => self.describe().0.to_owned(),
            (_, Some(step)) => step.content.clone(),
        }
    }

    fn describe(&self) -> (&'static str, &'static str) {
        match self {
            Self::Compacting => ("Compacting the conversation", "activity-compacting"),
            Self::Thinking => ("Thinking", "activity-thinking"),
            Self::Writing => ("Writing", "activity-writing"),
            Self::Waiting => ("Waiting for you", "activity-waiting"),
            Self::Working => ("Working", "activity-working"),
        }
    }
}

fn indicator(doing: &Doing, theme: &UiTheme) -> AnyElement {
    let (_, selector) = doing.describe();
    let mark = match doing {
        Doing::Waiting => div().size(rem(UNREAD_DOT_SIZE)).rounded_full().bg(theme.colors.warning).into_any_element(),
        _ => Waveform::new("activity-waveform", theme.colors.info).size(ICON_SIZE_SMALL).into_any_element(),
    };
    div()
        .debug_selector(|| "activity".to_owned())
        .flex_none()
        .size(rem(ICON_SIZE_SMALL))
        .flex()
        .items_center()
        .justify_center()
        .child(div().debug_selector(move || selector.to_owned()).flex().child(mark))
        .into_any_element()
}

fn plan_toggle(
    entries: &[acp::PlanEntry],
    open: bool,
    theme: &UiTheme,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let colors = theme.colors;
    let total = entries.len();
    let done = entries.iter().filter(|entry| is_plan_entry_finished(&entry.status)).count();
    let started = entries.iter().filter(|entry| entry.status == acp::PlanEntryStatus::InProgress).count();
    #[allow(clippy::cast_precision_loss, reason = "A plan's entries are far fewer than f32 counts exactly.")]
    let share = |count: usize| relative(count as f32 / total as f32);
    let bar = div()
        .flex_none()
        .w(rem(PLAN_BAR_WIDTH))
        .h(rem(PLAN_BAR_HEIGHT))
        .flex()
        .rounded_full()
        .overflow_hidden()
        .bg(colors.border_subtle)
        .child(div().h_full().w(share(done)).bg(colors.positive))
        .child(div().h_full().w(share(started)).bg(colors.accent));
    let chevron = if open { IconName::ChevronDown } else { IconName::ChevronRight };
    div()
        .relative()
        .flex_none()
        .children(open.then(|| plan_entries(entries, theme)))
        .child(
            div()
                .id("plan-toggle")
                .debug_selector(|| "plan-toggle".to_owned())
                .role(Role::Button)
                .aria_label(format!("Plan, {done} of {total} done"))
                .aria_expanded(open)
                .focusable()
                .tab_stop(true)
                .flex_none()
                .flex()
                .items_center()
                .gap_2()
                .cursor_pointer()
                .font_features(tabular_figures())
                .hover(move |toggle| toggle.text_color(colors.text))
                .focus_visible(move |toggle| toggle.text_color(colors.text))
                .on_click(on_toggle)
                .child(bar)
                .child(format!("{done}/{total}"))
                .child(Icon::new(chevron, colors.text_muted).size(ICON_SIZE_SMALL)),
        )
        .into_any_element()
}

fn plan_entries(entries: &[acp::PlanEntry], theme: &UiTheme) -> AnyElement {
    let colors = theme.colors;
    let rows = entries.iter().enumerate().map(|(index, entry)| {
        let (icon, color) = match entry.status {
            acp::PlanEntryStatus::InProgress => (IconName::CircleDot, colors.accent),
            acp::PlanEntryStatus::Completed => (IconName::CircleCheck, colors.positive),
            acp::PlanEntryStatus::Cancelled => (IconName::CircleSlash, colors.text_muted),
            _ => (IconName::Circle, colors.text_muted),
        };
        let done = is_plan_entry_finished(&entry.status);
        div()
            .debug_selector(move || format!("plan-entry-{index}"))
            .flex()
            .items_center()
            .gap_2()
            .child(Icon::new(icon, color).size(ICON_SIZE_SMALL))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(done, |text| text.line_through().text_color(colors.text_muted))
                    .child(entry.content.clone()),
            )
    });
    let panel = div()
        .debug_selector(|| "plan-panel".to_owned())
        .w(rem(PLAN_PANEL_WIDTH))
        .px_3()
        .py_2()
        .flex()
        .flex_col()
        .gap_1()
        .elevated(*theme)
        .shadow_lg()
        .text_color(colors.text_secondary)
        .children(rows);
    div()
        .absolute()
        .top_0()
        .right_0()
        .child(deferred(anchored().anchor(Anchor::BottomRight).snap_to_window().child(div().pb_2().child(panel))))
        .into_any_element()
}

fn elapsed(duration: Duration, theme: &UiTheme) -> AnyElement {
    div()
        .debug_selector(|| "elapsed".to_owned())
        .flex_none()
        .min_w(rem(ELAPSED_MIN_WIDTH))
        .text_right()
        .font_features(tabular_figures())
        .text_color(theme.colors.text_muted)
        .child(clock(duration))
        .into_any_element()
}

fn context_meter(usage: &acp::UsageUpdate, theme: &UiTheme) -> AnyElement {
    #[allow(clippy::cast_precision_loss, reason = "The ring only needs subpixel precision; labels use integer counts.")]
    let fraction = if usage.size == 0 { 0.0 } else { usage.used.min(usage.size) as f32 / usage.size as f32 };
    let color = if fraction >= 0.9 {
        theme.colors.destructive
    } else if fraction >= NEARLY_FULL {
        theme.colors.warning
    } else {
        theme.colors.accent
    };
    let label = if usage.size == 0 {
        format!("{} tokens", token_count(usage.used))
    } else {
        format!("{} / {} tokens", token_count(usage.used), token_count(usage.size))
    };
    let counts = (fraction >= NEARLY_FULL)
        .then(|| div().debug_selector(|| "context-tokens".to_owned()).text_color(color).child(label.clone()));
    let meter: Stateful<Div> = div()
        .id("context-meter")
        .debug_selector(|| "context-meter".to_owned())
        .role(Role::Meter)
        .aria_label(label.clone())
        .flex_none()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .gap_2()
        .text_color(theme.colors.text_muted)
        .font_features(tabular_figures())
        .tooltip(Tooltip::text(label, *theme))
        .child(context_ring(fraction, color, theme.colors.border))
        .children(counts);
    meter.into_any_element()
}

fn context_ring(fraction: f32, color: Hsla, track: Hsla) -> Div {
    div().debug_selector(|| "context-ring".to_owned()).size(rem(ICON_SIZE)).flex_none().child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let stroke = bounds.size.width * (RING_STROKE_WIDTH / ICON_SIZE);
                let radius = (bounds.size.width - stroke) / 2.0;
                let center = bounds.center();
                let at = |fraction: f32| {
                    let angle = fraction * TAU;
                    point(center.x + radius * angle.sin(), center.y - radius * angle.cos())
                };
                for (sweep, color) in [(1.0, track), (fraction, color)] {
                    if sweep == 0.0 {
                        continue;
                    }
                    let mut path = PathBuilder::stroke(stroke);
                    path.move_to(at(0.0));
                    path.arc_to(point(radius, radius), px(0.0), false, true, at(sweep.min(0.5)));
                    if sweep > 0.5 {
                        path.arc_to(point(radius, radius), px(0.0), false, true, at(sweep));
                    }
                    if sweep >= 1.0 {
                        path.close();
                    }
                    if let Ok(path) = path.build() {
                        window.paint_path(path, color);
                    }
                }
            },
        )
        .size_full(),
    )
}

fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn visible_plan(conversation: &Conversation) -> Option<&[acp::PlanEntry]> {
    let entries = conversation.plan()?.entries.as_slice();
    let finished = conversation.turn().is_idle() && entries.iter().all(|entry| is_plan_entry_finished(&entry.status));
    (!entries.is_empty() && !finished).then_some(entries)
}

fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".to_owned(), 1)]))
}

const PLAN_BAR_WIDTH: f32 = 32.0;
const PLAN_PANEL_WIDTH: f32 = 360.0;
const PLAN_BAR_HEIGHT: f32 = 4.0;
const RING_STROKE_WIDTH: f32 = 2.0;

const NEARLY_FULL: f32 = 0.75;

const ELAPSED_MIN_WIDTH: f32 = 36.0;
