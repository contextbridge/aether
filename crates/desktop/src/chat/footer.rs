use super::config_picker::ConfigPicker;
use crate::ui::UiTheme;
use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_CONTROL, ROW_HEIGHT, rem};
use crate::ui::{Icon, IconName, Typography, TypographyExt, plain_text};
use agent_client_protocol::schema::v2 as acp;
use client_core::session_config_view::LocalConfigOption;
use client_core::session_core::QueuedPrompt;
use gpui::{
    Anchor, AnyElement, App, Div, Entity, Hsla, PathBuilder, SharedString, Window, canvas, div, point, prelude::*, px,
};
use gpui_base::Popover;
use std::f32::consts::TAU;

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

/// How much of the model's context window the conversation fills, as a ring and token counts.
/// It is tagged `context-meter`.
pub(crate) fn context_meter(usage: &acp::UsageUpdate, theme: &UiTheme) -> Div {
    #[allow(clippy::cast_precision_loss, reason = "The ring only needs subpixel precision; labels use integer counts.")]
    let fraction = if usage.size == 0 { 0.0 } else { usage.used.min(usage.size) as f32 / usage.size as f32 };
    let color = if fraction >= 0.9 {
        theme.colors.destructive
    } else if fraction >= 0.75 {
        theme.colors.warning
    } else {
        theme.colors.accent
    };
    let label = if usage.size == 0 {
        format!("{} tokens", tokens(usage.used))
    } else {
        format!("{} / {} tokens", tokens(usage.used), tokens(usage.size))
    };
    div()
        .debug_selector(|| "context-meter".to_owned())
        .flex_none()
        .whitespace_nowrap()
        .flex()
        .items_center()
        .gap_2()
        .typography(Typography::Caption)
        .text_color(theme.colors.text_muted)
        .child(context_ring(fraction, color, theme.colors.border))
        .child(plain_text(label))
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

const RING_SIZE: f32 = 16.0;
const RING_STROKE_WIDTH: f32 = 2.0;
const MAX_QUEUED_ROWS: usize = 3;

fn context_ring(fraction: f32, color: Hsla, track: Hsla) -> Div {
    div().debug_selector(|| "context-ring".to_owned()).size(rem(RING_SIZE)).flex_none().child(
        canvas(
            |_, _, _| (),
            move |bounds, (), window, _| {
                let stroke = bounds.size.width * (RING_STROKE_WIDTH / RING_SIZE);
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

/// A token count as people read it: `850`, `12k`, `1.2M`.
fn tokens(count: u64) -> String {
    match count {
        0..1_000 => count.to_string(),
        1_000..1_000_000 => format!("{}k", count / 1_000),
        _ => format!("{}.{}M", count / 1_000_000, count / 100_000 % 10),
    }
}
