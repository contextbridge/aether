use crate::tasks::spawn_in;
use crate::threads::{Thread, ThreadStatus};
use crate::ui::tokens::{ICON_SIZE, ICON_SIZE_SMALL, ROW_HEIGHT, UNREAD_DOT_SIZE};
use crate::ui::{Hint, Icon, IconName, SurfaceExt, UiStyles, UiTheme, Waveform};
use acp_utils::conversation::{Activity, Conversation, is_plan_entry_finished};
use agent_client_protocol::v2::schema as acp;
use clankerdiff_rouge_shell::rouge::core::{
    AnyView, Cursor, Div, Path, Point, Styled, TextAlign, View, canvas, div, dynamic, pct, show, text, view,
};
use clankerdiff_rouge_shell::rouge::reactive::{Memo, Scope, Signal, effect, memo, signal, sleep, untrack};
use clankerdiff_rouge_shell::rouge::widgets::{Placement, popover};
use client_core::format::token_count;
use std::f32::consts::TAU;
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(crate) struct StatusLine {
    thread: Thread,
    running_since: Signal<Option<Instant>>,
    last_turn: Signal<Option<Duration>>,
    elapsed: Signal<Duration>,
    plan_open: Signal<bool>,
    scope: Scope,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Doing {
    Compacting,
    Thinking,
    Writing,
    Waiting,
    Working,
}

impl StatusLine {
    pub(crate) fn new(thread: Thread) -> Self {
        let status = Self {
            thread,
            running_since: signal(None),
            last_turn: signal(None),
            elapsed: signal(Duration::ZERO),
            plan_open: signal(false),
            scope: Scope::new(),
        };
        let running = memo(move || matches!(thread.status(), ThreadStatus::Running | ThreadStatus::NeedsInput));
        effect(move || {
            let running = running.get();
            untrack(|| status.sync(running));
        });
        status
    }

    fn sync(self, running: bool) {
        if !running {
            if let Some(since) = self.running_since.get_untracked() {
                self.running_since.set(None);
                self.last_turn.set(Some(since.elapsed()));
            }
            return;
        }
        if self.running_since.with_untracked(Option::is_some) {
            return;
        }
        let since = Instant::now();
        self.running_since.set(Some(since));
        self.elapsed.set(Duration::ZERO);
        spawn_in(self.scope, async move {
            loop {
                sleep(TICK).await;
                if !self.running_since.is_alive() || self.running_since.get_untracked() != Some(since) {
                    return;
                }
                self.elapsed.set(since.elapsed());
            }
        });
    }

    pub(crate) fn view(self) -> impl View {
        let thread = self.thread;
        let plan = memo(move || thread.with_core(|core| visible_plan(core.conversation())).flatten());
        let usage = memo(move || thread.with_core(|core| core.conversation().context_usage().cloned()).flatten());
        let doing = memo(move || {
            self.running_since.get()?;
            if thread.status() == ThreadStatus::NeedsInput {
                return Some(Doing::Waiting);
            }
            Some(thread.with_core(|core| Doing::of(core.conversation())).unwrap_or(Doing::Working))
        });
        let activity = memo(move || doing.get().is_some() || self.last_turn.get().is_some());
        let planned = memo(move || plan.with(Option::is_some));
        let timing = memo(move || self.running_since.with(Option::is_some));
        let metered = memo(move || usage.with(Option::is_some));
        let parts: [(Memo<bool>, AnyView); 4] = [
            (activity, self.activity(doing, plan).into_any()),
            (planned, self.plan(plan).into_any()),
            (timing, elapsed(self.elapsed).into_any()),
            (metered, context_meter(usage).into_any()),
        ];
        let visible = parts.each_ref().map(|(shown, _)| *shown);
        let parts = parts.into_iter().enumerate().map(move |(index, (shown, part))| {
            let separated = memo(move || visible[..index].iter().any(|shown| shown.get()));
            let part = std::cell::RefCell::new(Some(part));
            show(
                move || shown.get(),
                move || {
                    view! {
                        <div flex items_center gap=8.0 min_w=0.0 flex_shrink={if index == 0 { 1.0 } else { 0.0 }}>
                            <show when={move || separated.get()}>
                                <div flex_none><text muted>"·"</text></div>
                            </show>
                            {part.borrow_mut().take()}
                        </div>
                    }
                },
            )
        });
        view! {
            <div test_id="status" flex_1 min_w=0.0 h=ROW_HEIGHT flex items_center justify_end gap=8.0 body secondary>
                {..parts}
            </div>
        }
    }

    fn activity(self, doing: Memo<Option<Doing>>, plan: Memo<Option<Vec<acp::PlanEntry>>>) -> impl View {
        let theme = UiTheme::current();
        let label = move || doing.get().map(|doing| doing.label(plan.get().as_deref())).unwrap_or_default();
        let last_turn = memo(move || self.last_turn.get().filter(|_| doing.get().is_none()));
        let kind = memo(move || doing.get());
        view! {
            <div min_w=0.0 flex items_center gap=8.0>
                <dynamic>{kind.get().map(indicator)}</dynamic>
                <div test_id="status-label" min_w=0.0 overflow_hidden>
                    <text no_wrap color={move || {
                        if doing.get() == Some(Doing::Waiting) { theme.warning.get() } else { theme.text_secondary.get() }
                    }}>{label}</text>
                    <dynamic>{last_turn.get().map(|took| view! {
                        <div test_id="last-turn"><text muted no_wrap>{format!("Took {}", clock(took))}</text></div>
                    })}</dynamic>
                </div>
            </div>
        }
    }

    fn plan(self, plan: Memo<Option<Vec<acp::PlanEntry>>>) -> impl View {
        let open = self.plan_open;
        let theme = UiTheme::current();
        let toggle = dynamic(move || {
            let entries = plan.get().unwrap_or_default();
            let total = entries.len().max(1);
            let done = entries.iter().filter(|entry| is_plan_entry_finished(&entry.status)).count();
            let started = entries.iter().filter(|entry| entry.status == acp::PlanEntryStatus::InProgress).count();
            #[allow(clippy::cast_precision_loss, reason = "A plan's entries are far fewer than f32 counts exactly.")]
            let share = |count: usize| pct(count as f32 / total as f32 * 100.0);
            let chevron = if open.get() { IconName::ChevronDown } else { IconName::ChevronRight };
            view! {
                <div test_id="plan-toggle" flex_none flex items_center gap=8.0 cursor=Cursor::Pointer
                     on_click={move |event| {
                         event.stop_propagation();
                         open.update(|open| *open = !*open);
                     }}>
                    <div flex_none w=PLAN_BAR_WIDTH h=PLAN_BAR_HEIGHT flex rounded={PLAN_BAR_HEIGHT / 2.0}
                         overflow_hidden bg=theme.border_subtle>
                        <div h_full w={share(done)} bg=theme.positive/>
                        <div h_full w={share(started)} bg=theme.accent/>
                    </div>
                    <text>{format!("{done}/{}", entries.len())}</text>
                    {Icon::new(chevron, theme.text_muted).size(ICON_SIZE_SMALL)}
                </div>
            }
        });
        popover(open, toggle, move || plan_entries(&plan.get_untracked().unwrap_or_default()))
            .placement(Placement::Above)
    }
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

    fn label(self, plan: Option<&[acp::PlanEntry]>) -> String {
        let step =
            plan.and_then(|entries| entries.iter().find(|entry| entry.status == acp::PlanEntryStatus::InProgress));
        match (self, step) {
            (Self::Waiting | Self::Compacting, _) | (_, None) => self.describe().0.to_owned(),
            (_, Some(step)) => step.content.clone(),
        }
    }

    fn describe(self) -> (&'static str, &'static str) {
        match self {
            Self::Compacting => ("Compacting the conversation", "activity-compacting"),
            Self::Thinking => ("Thinking", "activity-thinking"),
            Self::Writing => ("Writing", "activity-writing"),
            Self::Waiting => ("Waiting for you", "activity-waiting"),
            Self::Working => ("Working", "activity-working"),
        }
    }
}

fn indicator(doing: Doing) -> impl View {
    let theme = UiTheme::current();
    let (_, id) = doing.describe();
    let mark = match doing {
        Doing::Waiting => {
            view! { <div size=UNREAD_DOT_SIZE rounded={UNREAD_DOT_SIZE / 2.0} bg=theme.warning/> }.into_any()
        }
        _ => Waveform::new("activity-waveform", theme.info).size(ICON_SIZE_SMALL).into_any(),
    };
    view! {
        <div test_id="activity" flex_none size=ICON_SIZE_SMALL flex items_center justify_center>
            <div test_id=id flex>{mark}</div>
        </div>
    }
}

fn plan_entries(entries: &[acp::PlanEntry]) -> Div {
    let theme = UiTheme::current();
    let rows = entries.iter().enumerate().map(|(index, entry)| {
        let (icon, color) = match entry.status {
            acp::PlanEntryStatus::InProgress => (IconName::CircleDot, theme.accent),
            acp::PlanEntryStatus::Completed => (IconName::CircleCheck, theme.positive),
            acp::PlanEntryStatus::Cancelled => (IconName::CircleSlash, theme.text_muted),
            _ => (IconName::Circle, theme.text_muted),
        };
        let done = is_plan_entry_finished(&entry.status);
        view! {
            <div test_id={format!("plan-entry-{index}")} flex items_center gap=8.0>
                {Icon::new(icon, color).size(ICON_SIZE_SMALL)}
                <div flex_1 min_w=0.0>
                    <text color={if done { theme.text_muted } else { theme.text_secondary }}>{entry.content.clone()}</text>
                </div>
            </div>
        }
    });
    view! {
        <div test_id="plan-panel" w=PLAN_PANEL_WIDTH px=12.0 py=8.0 flex col gap=4.0 floating>{..rows}</div>
    }
}

fn elapsed(elapsed: Signal<Duration>) -> impl View {
    view! {
        <div test_id="elapsed" flex_none min_w=ELAPSED_MIN_WIDTH>
            <text muted align=TextAlign::End>{move || clock(elapsed.get())}</text>
        </div>
    }
}

fn context_meter(usage: Memo<Option<acp::UsageUpdate>>) -> impl View {
    let theme = UiTheme::current();
    dynamic(move || {
        let usage = usage.get()?;
        #[allow(
            clippy::cast_precision_loss,
            reason = "The ring only needs subpixel precision; labels use integer counts."
        )]
        let fraction = if usage.size == 0 { 0.0 } else { usage.used.min(usage.size) as f32 / usage.size as f32 };
        let color = if fraction >= FULL {
            theme.destructive
        } else if fraction >= NEARLY_FULL {
            theme.warning
        } else {
            theme.accent
        };
        let label = if usage.size == 0 {
            format!("{} tokens", token_count(usage.used))
        } else {
            format!("{} / {} tokens", token_count(usage.used), token_count(usage.size))
        };
        let counts = (fraction >= NEARLY_FULL).then(|| {
            view! { <div test_id="context-tokens"><text no_wrap color=color>{label.clone()}</text></div> }
        });
        let track = theme.border;
        let ring = canvas(move |painter| {
            let size = painter.size();
            let radius = size.width.min(size.height) / 2.0;
            let stroke = size.width * (RING_STROKE_WIDTH / ICON_SIZE);
            let center = Point::new(size.width / 2.0, size.height / 2.0);
            let inner = radius - stroke / 2.0;
            painter.stroke(&arc(center, inner, 1.0), stroke, track.get());
            if fraction > 0.0 {
                painter.stroke(&arc(center, inner, fraction), stroke, color.get());
            }
        })
        .size_full();
        Some(
            view! {
                <div test_id="context-meter" flex_none flex items_center gap=8.0>
                    <div test_id="context-ring" size=ICON_SIZE flex_none>{ring}</div>
                    {counts}
                </div>
            }
            .tooltip(label),
        )
    })
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "An arc has at most ARC_STEPS steps, which f32 counts exactly."
)]
fn arc(center: Point, radius: f32, sweep: f32) -> Path {
    let steps = (ARC_STEPS as f32 * sweep).ceil().max(1.0) as usize;
    let point = |step: usize| {
        let angle = TAU * sweep * step as f32 / steps as f32 - TAU / 4.0;
        (center.x + radius * angle.cos(), center.y + radius * angle.sin())
    };
    let (x, y) = point(0);
    (1..=steps)
        .fold(Path::builder().move_to(x, y), |path, step| {
            let (x, y) = point(step);
            path.line_to(x, y)
        })
        .build()
}

fn clock(duration: Duration) -> String {
    let seconds = duration.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

fn visible_plan(conversation: &Conversation) -> Option<Vec<acp::PlanEntry>> {
    let entries = conversation.plan()?.entries.as_slice();
    let finished = conversation.turn().is_idle() && entries.iter().all(|entry| is_plan_entry_finished(&entry.status));
    (!entries.is_empty() && !finished).then(|| entries.to_vec())
}

const TICK: Duration = Duration::from_secs(1);
const PLAN_BAR_WIDTH: f32 = 32.0;
const PLAN_PANEL_WIDTH: f32 = 360.0;
const PLAN_BAR_HEIGHT: f32 = 4.0;
const RING_STROKE_WIDTH: f32 = 2.0;
const ARC_STEPS: usize = 48;
const NEARLY_FULL: f32 = 0.75;
const FULL: f32 = 0.9;
const ELAPSED_MIN_WIDTH: f32 = 36.0;
