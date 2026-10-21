use crate::threads::Thread;
use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_CONTAINER, RADIUS_CONTROL, ROW_HEIGHT};
use crate::ui::{Icon, IconName, ListItem, Typography, TypographyExt};
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_gpui::ui::prelude::UiTheme;
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::QueuedPrompt;
use gpui::{Anchor, AnyElement, Div, Entity, SharedString, div, prelude::*, px};
use gpui_base::{Popover, Selectable as _};

/// A menu for one of the session's options, such as its model, mode or reasoning effort, that
/// shows the current choice. Choosing another asks the agent to switch.
///
/// The menu's button is tagged `config-<option id>` and each choice `config-<option id>-<value>`.
pub(crate) fn config_selector(
    option: &LocalConfigOption,
    thread: &Entity<Thread>,
    theme: &UiTheme,
) -> Option<AnyElement> {
    let select = option.select()?;
    let current = select.values.iter().find(|value| value.value == select.current_value);
    let label = current.map_or(select.current_value, |value| value.name.as_str()).to_owned();
    let selector = format!("config-{}", option.id);
    let trigger = div()
        .id(SharedString::from(selector.clone()))
        .debug_selector({
            let selector = selector.clone();
            move || selector
        })
        .h(px(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .rounded(px(RADIUS_CONTROL))
        .typography(Typography::Small)
        .text_color(theme.colors.text_secondary)
        .cursor_pointer()
        .hover(|trigger| trigger.bg(theme.colors.surface_hover))
        .child(label)
        .child(Icon::new(IconName::ChevronDown, theme.colors.text_muted).size(ICON_SIZE_SMALL));
    let name = option.name.clone();
    let config_id = option.id.clone();
    let values = select.values.to_vec();
    let current_value = select.current_value.to_owned();
    let thread = thread.clone();
    let theme = *theme;
    let popover = Popover::new(SharedString::from(format!("{selector}-popover")))
        .anchor(Anchor::BottomLeft)
        .trigger_with(move |open, _, _| {
            trigger.when(open, |trigger| trigger.bg(theme.colors.surface_selected)).into_any_element()
        })
        .content(move |_, _, cx| {
            let popover = cx.entity();
            let mut entries = div().p_1().flex().flex_col().child(
                div()
                    .px_2()
                    .py_1()
                    .typography(Typography::Label)
                    .text_color(theme.colors.text_muted)
                    .child(name.clone()),
            );
            for value in &values {
                let entry_selector = format!("{selector}-{}", value.value);
                let current = value.value == current_value;
                let mut row = ListItem::new(entry_selector, theme)
                    .selected(current)
                    .child(div().flex_1().min_w_0().truncate().child(value.name.clone()));
                if current {
                    row = row.child(Icon::new(IconName::Check, theme.colors.text_muted));
                }
                if value.is_disabled {
                    row = row.child(
                        div().typography(Typography::Caption).text_color(theme.colors.text_muted).child("unavailable"),
                    );
                } else {
                    let (popover, thread, config_id, value) =
                        (popover.clone(), thread.clone(), config_id.clone(), value.value.clone());
                    row = row.on_click(move |_, window, cx| {
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        thread.update(cx, |thread, cx| thread.set_config_option(&config_id, &value, cx));
                    });
                }
                entries = entries.child(row);
            }
            div()
                .w(px(MENU_WIDTH))
                .rounded(px(RADIUS_CONTAINER))
                .border_1()
                .border_color(theme.colors.border)
                .bg(theme.colors.surface)
                .shadow_lg()
                .child(entries)
        });
    Some(popover.into_any_element())
}

/// How much of the model's context window the conversation fills, as a bar and a percentage.
/// It is tagged `context-meter`.
pub(crate) fn context_meter(usage: &acp::UsageUpdate, theme: &UiTheme) -> Div {
    let percent = usage.used.min(usage.size).checked_mul(100).and_then(|used| used.checked_div(usage.size));
    let percent = percent.and_then(|percent| u8::try_from(percent).ok()).unwrap_or(0);
    let fraction = f32::from(percent) / 100.0;
    let color = if fraction >= 0.9 {
        theme.colors.destructive
    } else if fraction >= 0.75 {
        theme.colors.warning
    } else {
        theme.colors.accent
    };
    div()
        .debug_selector(|| "context-meter".to_owned())
        .flex()
        .items_center()
        .gap_2()
        .typography(Typography::Caption)
        .text_color(theme.colors.text_muted)
        .child(
            div()
                .w(px(METER_WIDTH))
                .h(px(METER_HEIGHT))
                .rounded_full()
                .bg(theme.colors.border)
                .child(div().h_full().rounded_full().bg(color).w(px(METER_WIDTH * fraction))),
        )
        .child(format!("{percent}% of {}", tokens(usage.size)))
}

/// The prompts the agent has not taken up yet, oldest first, with any beyond the first few
/// counted. It is tagged `queued-prompts`.
pub(crate) fn queued_prompts(prompts: &[QueuedPrompt], theme: &UiTheme) -> Div {
    let shown = if prompts.len() > MAX_QUEUED_ROWS + 1 { MAX_QUEUED_ROWS } else { prompts.len() };
    let mut list =
        div().debug_selector(|| "queued-prompts".to_owned()).flex().flex_col().gap_1().typography(Typography::Small);
    for prompt in &prompts[..shown] {
        let text = prompt.submission.text.lines().next().unwrap_or_default().to_owned();
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .min_w_0()
                .child(Icon::new(IconName::ChevronRight, theme.colors.text_muted).size(ICON_SIZE_SMALL))
                .child(div().flex_none().text_color(theme.colors.text_muted).child("Queued"))
                .child(div().flex_1().min_w_0().truncate().text_color(theme.colors.text_secondary).child(text)),
        );
    }
    if shown < prompts.len() {
        list = list
            .child(div().text_color(theme.colors.text_muted).child(format!("{} more queued", prompts.len() - shown)));
    }
    list
}

const MENU_WIDTH: f32 = 240.0;
const METER_WIDTH: f32 = 48.0;
const METER_HEIGHT: f32 = 4.0;
const MAX_QUEUED_ROWS: usize = 3;

/// A token count as people read it: `850`, `12k`, `1.2M`.
fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{}.{}M", count / 1_000_000, count / 100_000 % 10),
    }
}
