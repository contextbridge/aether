use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_CONTROL, rem};
use crate::ui::{Icon, IconButton, IconName, Typography, TypographyExt, UiTheme};
use client_core::composer::attachment::{AttachmentKind, PromptAttachment, classify_attachment};
use gpui::{App, ClickEvent, Div, Image, ImageFormat, Window, div, prelude::*};
use std::fs;
use utils::temp_dir::TempDir;

#[derive(Default)]
pub(crate) struct PastedImages {
    dir: TempDir,
    saved: usize,
}

impl PastedImages {
    pub(crate) fn save(&mut self, image: &Image) -> Result<PromptAttachment, String> {
        let extension = match image.format {
            ImageFormat::Png => "png",
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
            other => {
                let format = format!("{other:?}").to_uppercase();
                return Err(format!("Can't attach {format} images; paste a PNG, JPEG, GIF or WebP instead."));
            }
        };
        self.saved += 1;
        let display_name = format!("pasted-image-{}.{extension}", self.saved);
        let path = self.dir.path().join(&display_name);
        fs::create_dir_all(self.dir.path())
            .and_then(|()| fs::write(&path, &image.bytes))
            .map_err(|error| format!("Couldn't save the pasted image: {error}"))?;
        Ok(PromptAttachment { path, display_name })
    }
}

pub(crate) fn attachment_chip(
    index: usize,
    attachment: &PromptAttachment,
    theme: &UiTheme,
    on_remove: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Div {
    let icon = match classify_attachment(&attachment.path) {
        AttachmentKind::Audio => IconName::AudioLines,
        _ => IconName::Image,
    };
    div()
        .debug_selector(move || format!("composer-attachment-{index}"))
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .pl_2()
        .rounded(rem(RADIUS_CONTROL))
        .bg(theme.colors.surface_selected)
        .typography(Typography::Small)
        .text_color(theme.colors.text_secondary)
        .child(Icon::new(icon, theme.colors.text_muted).size(ICON_SIZE_SMALL))
        .child(div().min_w_0().truncate().child(attachment.display_name.clone()))
        .child(
            IconButton::new(format!("composer-attachment-{index}-remove"), IconName::X, "Remove", *theme)
                .on_click(on_remove),
        )
}
