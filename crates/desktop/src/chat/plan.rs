use crate::theme::DesktopTheme;
use crate::threads::Thread;
use acp_utils::conversation::Conversation;
use agent_client_protocol::schema::v2 as acp;
use client_core::session_core::SessionCore;
use gpui::{Context, Entity, FontWeight, Subscription, Window, div, prelude::*};

/// The agent's plan for its turn, kept in view above the composer. It shows every entry with its
/// progress, and folds down to a one-line count when clicked.
///
/// It is tagged `plan-panel`, its header `plan-header`, and each entry `plan-entry-<n>`. Once
/// every entry is done and the turn has ended, it hides.
pub(crate) struct PlanPanel {
    thread: Entity<Thread>,
    collapsed: bool,
    _subscription: Subscription,
}

impl PlanPanel {
    pub(crate) fn new(thread: Entity<Thread>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&thread, |_, _, cx| cx.notify());
        Self { thread, collapsed: false, _subscription: subscription }
    }
}

impl Render for PlanPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = DesktopTheme::global(cx).ui();
        let conversation = self.thread.read(cx).core().map(SessionCore::conversation);
        let Some(entries) = conversation.and_then(visible_plan) else {
            return div();
        };
        let done = entries.iter().filter(|entry| is_done(&entry.status)).count();
        let collapsed = self.collapsed;
        let header = div()
            .id("plan-header")
            .debug_selector(|| "plan-header".to_owned())
            .flex()
            .gap_2()
            .cursor_pointer()
            .text_color(theme.colors.text_secondary)
            .on_click(cx.listener(|this, _, _, cx| {
                this.collapsed = !this.collapsed;
                cx.notify();
            }))
            .child(if collapsed { "▸" } else { "▾" })
            .child(div().font_weight(FontWeight::SEMIBOLD).child("Plan"))
            .child(div().text_color(theme.colors.text_muted).child(format!("{done} of {} done", entries.len())));
        let mut panel = div()
            .debug_selector(|| "plan-panel".to_owned())
            .flex_none()
            .w_full()
            .px_4()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .border_t_1()
            .border_color(theme.colors.border)
            .text_sm()
            .child(header);
        if !collapsed {
            for (index, entry) in entries.iter().enumerate() {
                let (glyph, color) = match entry.status {
                    acp::PlanEntryStatus::InProgress => ("◐", theme.colors.accent),
                    acp::PlanEntryStatus::Completed => ("✓", theme.colors.positive),
                    acp::PlanEntryStatus::Cancelled => ("⊘", theme.colors.text_muted),
                    _ => ("○", theme.colors.text_muted),
                };
                let done = is_done(&entry.status);
                panel = panel.child(
                    div()
                        .debug_selector(move || format!("plan-entry-{index}"))
                        .flex()
                        .gap_2()
                        .child(div().flex_none().text_color(color).child(glyph))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .when(done, |text| text.line_through().text_color(theme.colors.text_muted))
                                .child(entry.content.clone()),
                        ),
                );
            }
        }
        panel
    }
}

/// The plan's entries, unless there is no plan or it is finished: every entry done and the turn
/// over.
fn visible_plan(conversation: &Conversation) -> Option<&[acp::PlanEntry]> {
    let entries = conversation.plan()?.entries.as_slice();
    let finished = conversation.turn().is_idle() && entries.iter().all(|entry| is_done(&entry.status));
    (!entries.is_empty() && !finished).then_some(entries)
}

fn is_done(status: &acp::PlanEntryStatus) -> bool {
    matches!(status, acp::PlanEntryStatus::Completed | acp::PlanEntryStatus::Cancelled)
}
