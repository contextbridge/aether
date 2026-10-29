use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_INNER, rem};
use crate::ui::{Icon, IconName, Typography, TypographyExt, UiTheme};
use agent_client_protocol::schema::v2 as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use client_core::composer::attachment::{AttachmentKind, PromptAttachment, classify_attachment};
use gpui::{Div, Image, ImageFormat, ImageSource, ObjectFit, StyledImage, div, img, prelude::*};
use std::sync::Arc;

pub(crate) enum Attachment {
    Image(Arc<Image>),
    File { icon: IconName, name: String },
}

impl Attachment {
    pub(crate) fn of(block: &acp::ContentBlock) -> Option<Self> {
        let file = |icon, name: &str| Some(Self::File { icon, name: name.to_owned() });
        match block {
            acp::ContentBlock::Text(_) => None,
            acp::ContentBlock::Image(image) => {
                decode(image).map(Self::Image).or_else(|| file(IconName::Image, "Image"))
            }
            acp::ContentBlock::Audio(_) => file(IconName::AudioLines, "Audio"),
            acp::ContentBlock::ResourceLink(link) => file(IconName::File, &link.name),
            acp::ContentBlock::Resource(resource) => file(IconName::File, file_name(resource)),
            _ => file(IconName::File, "Attachment"),
        }
    }
}

pub(crate) fn chip(selector: String, preview: impl IntoElement, name: &str, theme: &UiTheme) -> Div {
    div()
        .debug_selector(move || selector)
        .flex()
        .items_center()
        .gap_1()
        .min_w_0()
        .px_2()
        .py_1()
        .rounded(rem(RADIUS_INNER))
        .bg(theme.colors.surface_selected)
        .typography(Typography::Small)
        .text_color(theme.colors.text_secondary)
        .child(preview)
        .child(div().min_w_0().truncate().child(name.to_owned()))
}

pub(crate) fn chip_icon(icon: IconName, theme: &UiTheme) -> Icon {
    Icon::new(icon, theme.colors.text_muted).size(ICON_SIZE_SMALL)
}

pub(crate) fn thumbnail(selector: String, image: impl Into<ImageSource>, size: f32, theme: &UiTheme) -> Div {
    div()
        .debug_selector(move || selector)
        .flex_none()
        .size(rem(size))
        .rounded(rem(RADIUS_INNER))
        .overflow_hidden()
        .border_1()
        .border_color(theme.colors.border)
        .child(img(image).size_full().object_fit(ObjectFit::Cover))
}

pub(crate) fn preview(selector: String, attachment: &PromptAttachment, size: f32, theme: &UiTheme) -> Div {
    match classify_attachment(&attachment.path) {
        AttachmentKind::Audio => div().flex().child(chip_icon(IconName::AudioLines, theme)),
        _ => thumbnail(selector, attachment.path.clone(), size, theme),
    }
}

fn decode(image: &acp::ImageContent) -> Option<Arc<Image>> {
    let format = ImageFormat::from_mime_type(&image.mime_type.0)?;
    let bytes = BASE64.decode(&image.data).ok()?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
}

fn file_name(resource: &acp::EmbeddedResource) -> &str {
    let uri = match &resource.resource {
        acp::EmbeddedResourceResource::TextResourceContents(text) => text.uri.as_str(),
        acp::EmbeddedResourceResource::BlobResourceContents(blob) => blob.uri.as_str(),
        _ => "Attachment",
    };
    uri.rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or(uri)
}
