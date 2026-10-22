use crate::theme::DesktopTheme;
use crate::threads::Thread;
use crate::ui::tokens::{CONTENT_MAX_WIDTH, ICON_SIZE_SMALL, RADIUS_CONTAINER, rem};
use crate::ui::{Icon, IconName, Typography, TypographyExt};
use acp_utils::conversation::Conversation;
use agent_client_protocol::schema::v2 as acp;
use client_core::session_core::SessionCore;
use gpui::{Context, Entity, Subscription, Window, div, prelude::*};

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
        let colors = theme.colors;
        let conversation = self.thread.read(cx).core().map(SessionCore::conversation);
        let Some(entries) = conversation.and_then(visible_plan) else {
            return div();
        };
        let done = entries.iter().filter(|entry| is_done(&entry.status)).count();
        let collapsed = self.collapsed;
        let chevron = if collapsed { IconName::ChevronRight } else { IconName::ChevronDown };
        let header = div()
            .id("plan-header")
            .debug_selector(|| "plan-header".to_owned())
            .flex()
            .items_center()
            .gap_2()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.collapsed = !this.collapsed;
                cx.notify();
            }))
            .child(Icon::new(chevron, colors.text_muted).size(ICON_SIZE_SMALL))
            .child(div().typography(Typography::Label).text_color(colors.text_secondary).child("Plan"))
            .child(
                div()
                    .typography(Typography::Caption)
                    .text_color(colors.text_muted)
                    .child(format!("{done} of {} done", entries.len())),
            );
        let mut card = div()
            .w_full()
            .max_w(rem(CONTENT_MAX_WIDTH))
            .mx_auto()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_1()
            .rounded(rem(RADIUS_CONTAINER))
            .border_1()
            .border_color(colors.border)
            .bg(colors.surface)
            .typography(Typography::Small)
            .child(header);
        if !collapsed {
            for (index, entry) in entries.iter().enumerate() {
                let (icon, color) = match entry.status {
                    acp::PlanEntryStatus::InProgress => (IconName::CircleDot, colors.accent),
                    acp::PlanEntryStatus::Completed => (IconName::CircleCheck, colors.positive),
                    acp::PlanEntryStatus::Cancelled => (IconName::CircleSlash, colors.text_muted),
                    _ => (IconName::Circle, colors.text_muted),
                };
                let done = is_done(&entry.status);
                card = card.child(
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
                        ),
                );
            }
        }
        div().debug_selector(|| "plan-panel".to_owned()).flex_none().w_full().px_4().pt_2().child(card)
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
