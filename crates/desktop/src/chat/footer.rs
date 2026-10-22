use super::config_picker::ConfigPicker;
use crate::ui::UiTheme;
use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_CONTROL, ROW_HEIGHT, rem};
use crate::ui::{Icon, IconName, Typography, TypographyExt};
use agent_client_protocol::schema::v2 as acp;
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::QueuedPrompt;
use gpui::{Anchor, AnyElement, App, Div, Entity, SharedString, Window, div, prelude::*};
use gpui_base::Popover;

/// The button for one of the session's options, such as its model, mode or reasoning effort,
/// that shows the current choice and opens `picker` above itself while there is one.
///
/// Clicking it asks `on_open_change` to open the picker, and clicking it again or anywhere
/// outside the picker asks it to close. The button is tagged `config-<option id>`.
pub(crate) fn config_selector(
    option: &LocalConfigOption,
    picker: Option<Entity<ConfigPicker>>,
    theme: &UiTheme,
    on_open_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
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
        .h(rem(ROW_HEIGHT))
        .flex()
        .items_center()
        .gap_1()
        .px_2()
        .rounded(rem(RADIUS_CONTROL))
        .typography(Typography::Small)
        .text_color(theme.colors.text_secondary)
        .cursor_pointer()
        .hover(|trigger| trigger.bg(theme.colors.surface_hover))
        .child(label)
        .child(Icon::new(IconName::ChevronDown, theme.colors.text_muted).size(ICON_SIZE_SMALL));
    let theme = *theme;
    let open = picker.is_some();
    let popover = Popover::new(SharedString::from(format!("{selector}-popover")))
        .anchor(Anchor::BottomLeft)
        .open(open)
        .on_open_change(on_open_change)
        .trigger_with(move |open, _, _| {
            trigger.when(open, |trigger| trigger.bg(theme.colors.surface_selected)).into_any_element()
        })
        .content(move |_, _, _| div().children(picker));
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
                .w(rem(METER_WIDTH))
                .h(rem(METER_HEIGHT))
                .rounded_full()
                .bg(theme.colors.border)
                .child(div().h_full().rounded_full().bg(color).w(rem(METER_WIDTH * fraction))),
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
