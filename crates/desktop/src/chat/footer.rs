use crate::threads::Thread;
use agent_client_protocol::schema::v2 as acp;
use clankerdiff_gpui::ui::prelude::{ListRow, MutedText, SelectionState, Surface, UiTheme};
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::QueuedPrompt;
use gpui::{Anchor, AnyElement, Div, Entity, SharedString, div, prelude::*, px};
use gpui_base::Popover;

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
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .py_0p5()
        .rounded_md()
        .text_sm()
        .text_color(theme.colors.text_secondary)
        .cursor_pointer()
        .hover(|trigger| trigger.bg(theme.colors.surface_hover))
        .child(label)
        .child(div().text_xs().child("▾"));
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
            let mut entries =
                div().py_1().child(div().px_3().py_1().text_xs().child(MutedText::new(name.clone(), theme)));
            for value in &values {
                let entry_selector = format!("{selector}-{}", value.value);
                let state = if value.value == current_value { SelectionState::Selected } else { SelectionState::None };
                let mut row = ListRow::new(SharedString::from(entry_selector.clone()), theme)
                    .state(state)
                    .child(div().flex_1().min_w_0().truncate().child(value.name.clone()));
                if value.is_disabled {
                    row = row.child(MutedText::new("unavailable", theme));
                } else {
                    let (popover, thread, config_id, value) =
                        (popover.clone(), thread.clone(), config_id.clone(), value.value.clone());
                    row = row.on_click(move |_, window, cx| {
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        thread.update(cx, |thread, cx| thread.set_config_option(&config_id, &value, cx));
                    });
                }
                entries = entries.child(div().debug_selector(move || entry_selector).child(row));
            }
            div().w(px(MENU_WIDTH)).child(Surface::new(theme).padded(false).raised(true).child(entries))
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
        .text_sm()
        .text_color(theme.colors.text_muted)
        .child(
            div()
                .w(px(METER_WIDTH))
                .h(px(4.0))
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
    let mut list = div().debug_selector(|| "queued-prompts".to_owned()).flex().flex_col().text_sm();
    for prompt in &prompts[..shown] {
        let text = prompt.submission.text.lines().next().unwrap_or_default().to_owned();
        list = list.child(
            div()
                .flex()
                .gap_2()
                .min_w_0()
                .child(div().flex_none().child(MutedText::new("Queued ›", *theme)))
                .child(div().flex_1().min_w_0().truncate().text_color(theme.colors.text_secondary).child(text)),
        );
    }
    if shown < prompts.len() {
        list = list.child(MutedText::new(format!("+{} more queued", prompts.len() - shown), *theme));
    }
    list
}

const MENU_WIDTH: f32 = 240.0;
const METER_WIDTH: f32 = 48.0;
const MAX_QUEUED_ROWS: usize = 3;

/// A token count as people read it: `850`, `12k`, `1.2M`.
fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{}.{}M", count / 1_000_000, count / 100_000 % 10),
    }
}
