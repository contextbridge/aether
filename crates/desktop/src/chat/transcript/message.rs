use crate::chat::attachment::{Attachment, chip, chip_icon, thumbnail};
use crate::threads::{Thread, ThreadStatus};
use crate::ui::{
    Backdrop, Button, ButtonVariant, Disclosure, Icon, IconButton, IconName, ListRow, StyledExt, SurfaceExt,
    Typography, TypographyExt, UiTheme,
};
use clankerdiff_gpui::DiffViewer;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Div, Entity, Hsla, Image, ObjectFit, SharedString, StyledImage, Window,
    div, img, prelude::*, relative,
};
use std::rc::Rc;
use std::sync::Arc;

pub(crate) fn prompt(
    row: usize,
    text: String,
    attachments: &[(usize, Attachment)],
    theme: &UiTheme,
    on_open: impl Fn(&Arc<Image>, &mut Window, &mut App) + 'static,
) -> Div {
    let on_open = Rc::new(on_open);
    let attachments: Vec<AnyElement> = attachments
        .iter()
        .map(|(block, attachment)| {
            let selector = format!("message-attachment-{row}-{block}");
            match attachment {
                Attachment::Image(image) => {
                    let (image, on_open) = (image.clone(), on_open.clone());
                    thumbnail(selector.clone(), image.clone(), THUMBNAIL_SIZE, theme)
                        .id(SharedString::from(selector))
                        .cursor_pointer()
                        .on_click(move |_: &ClickEvent, window, cx| on_open(&image, window, cx))
                        .into_any_element()
                }
                Attachment::File { icon, name } => {
                    chip(selector, chip_icon(*icon, theme), name, theme).into_any_element()
                }
            }
        })
        .collect();
    div()
        .max_w(relative(PROMPT_MAX_WIDTH))
        .px_4()
        .py_2()
        .flex()
        .flex_col()
        .gap_2()
        .elevated(*theme)
        .typography(Typography::Reading)
        .when(!text.is_empty(), |bubble| bubble.child(text))
        .when(!attachments.is_empty(), |bubble| bubble.child(div().flex().flex_wrap().gap_2().children(attachments)))
}

pub(crate) fn message_actions(row: usize, text: String, theme: &UiTheme) -> Div {
    div().pt_1().flex().reveal_on_hover(row_group(row)).child(
        IconButton::new(format!("copy-message-{row}"), IconName::Copy, "Copy", *theme)
            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))),
    )
}

pub(crate) fn row_group(row: usize) -> SharedString {
    ListRow::hover_group(&format!("transcript-row-{row}"))
}

pub(crate) fn image_preview(
    image: Arc<Image>,
    theme: &UiTheme,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> Backdrop {
    Backdrop::new("image-preview", *theme)
        .on_dismiss(on_close)
        .child(img(image).size_full().object_fit(ObjectFit::Contain))
}

pub(crate) fn full_diff(
    viewer: Entity<DiffViewer>,
    theme: &UiTheme,
    on_close: impl Fn(&mut Window, &mut App) + 'static,
) -> Backdrop {
    let on_close = Rc::new(on_close);
    let close = on_close.clone();
    Backdrop::new("full-diff", *theme).on_dismiss(move |window, cx| on_close(window, cx)).child(
        div()
            .debug_selector(|| "full-diff-panel".to_owned())
            .occlude()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .floating(*theme)
            .child(
                div().flex().justify_end().p_1().child(
                    IconButton::new("close-full-diff", IconName::X, "Close", *theme)
                        .on_click(move |_, window, cx| close(window, cx)),
                ),
            )
            .child(div().flex_1().min_h_0().child(viewer)),
    )
}

pub(crate) fn thinking_block(
    text: String,
    streaming: bool,
    index: usize,
    expanded: bool,
    theme: &UiTheme,
    on_toggle: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let colors = theme.colors;
    div()
        .flex()
        .flex_col()
        .gap_2()
        .typography(Typography::Body)
        .text_color(colors.text_muted)
        .child(Disclosure::new(("thinking", index), expanded, *theme).on_toggle(on_toggle).child(if streaming {
            "Thinking…"
        } else {
            "Thought"
        }))
        .when(expanded, |block| {
            block.child(
                div()
                    .debug_selector(move || format!("thinking-text-{index}"))
                    .pl_3()
                    .border_l_2()
                    .border_color(colors.border_subtle)
                    .child(text),
            )
        })
        .into_any_element()
}

pub(crate) fn notice(text: &str, theme: &UiTheme) -> AnyElement {
    line(IconName::Info, theme.colors.text_muted, text).into_any_element()
}

pub(crate) fn error(index: usize, text: &str, theme: &UiTheme) -> AnyElement {
    line(IconName::TriangleAlert, theme.colors.destructive, text)
        .debug_selector(move || format!("transcript-error-{index}"))
        .into_any_element()
}

pub(crate) fn reconnect_banner(thread: &Entity<Thread>, status: ThreadStatus, theme: &UiTheme) -> Option<Div> {
    let problem = status.problem()?;
    Some(
        div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .elevated(*theme)
            .child(line(IconName::TriangleAlert, theme.colors.warning, problem).flex_1().min_w_0())
            .child(reconnect_button(thread, theme)),
    )
}

pub(crate) fn reconnect_button(thread: &Entity<Thread>, theme: &UiTheme) -> Button {
    let thread = thread.clone();
    Button::new("transcript-reconnect", "Reconnect", *theme)
        .icon(IconName::RotateCw)
        .variant(ButtonVariant::Primary)
        .on_click(move |_, _, cx| thread.update(cx, Thread::reconnect))
}

const THUMBNAIL_SIZE: f32 = 96.0;

const PROMPT_MAX_WIDTH: f32 = 0.85;

fn line(icon: IconName, color: Hsla, text: &str) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .typography(Typography::Small)
        .text_color(color)
        .child(Icon::new(icon, color))
        .child(div().flex_1().min_w_0().child(text.to_owned()))
}
