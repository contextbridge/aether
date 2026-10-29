use crate::chat::attachment::{Attachment, chip, chip_icon, thumbnail};
use crate::ui::{Backdrop, IconButton, IconName, ListRow, StyledExt, SurfaceExt, Typography, TypographyExt, UiTheme};
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Div, Image, ObjectFit, SharedString, StyledImage, Window, div, img,
    prelude::*, relative,
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

const THUMBNAIL_SIZE: f32 = 96.0;

const PROMPT_MAX_WIDTH: f32 = 0.85;
