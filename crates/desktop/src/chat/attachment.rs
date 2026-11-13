use crate::ui::tokens::{ICON_SIZE_SMALL, RADIUS_INNER};
use crate::ui::{Icon, IconName, Theme, UiStyles};
use agent_client_protocol::v2::schema as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use client_core::composer::attachment::{AttachmentKind, PromptAttachment, classify_attachment};
use rouge::core::{AnyView, Div, Fit, ImageSource, Styled, View, div, image, px, text, view};
use std::sync::Arc;

#[derive(Clone, PartialEq)]
pub(crate) enum Attachment {
    Image(Arc<[u8]>),
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

pub(crate) fn chip(id: String, preview: impl View, name: &str) -> Div {
    let theme = Theme::current();
    view! {
        <div test_id=id flex items_center gap=4.0 min_w=0.0 px=8.0 py=4.0 rounded=RADIUS_INNER
             bg=theme.surface_selected>
            {preview}
            <div min_w=0.0 overflow_hidden><text small secondary no_wrap>{name.to_owned()}</text></div>
        </div>
    }
}

pub(crate) fn chip_icon(icon: IconName) -> Icon {
    Icon::new(icon, Theme::current().text_muted).size(ICON_SIZE_SMALL)
}

pub(crate) fn thumbnail(id: String, source: ImageSource, size: f32) -> Div {
    view! {
        <div test_id=id flex_none size=size rounded=RADIUS_INNER overflow_hidden border=px(1.0)
             border_color=Theme::current().border>
            {image(source).fit(Fit::Cover).size_full()}
        </div>
    }
}

pub(crate) fn preview(id: String, attachment: &PromptAttachment, size: f32) -> AnyView {
    match classify_attachment(&attachment.path) {
        AttachmentKind::Audio => div().flex().child(chip_icon(IconName::AudioLines)).into_any(),
        _ => thumbnail(id, ImageSource::File(attachment.path.clone()), size).into_any(),
    }
}

fn decode(image: &acp::ImageContent) -> Option<Arc<[u8]>> {
    matches!(&*image.mime_type.0, "image/png" | "image/jpeg" | "image/jpg")
        .then(|| BASE64.decode(&image.data).ok())
        .flatten()
        .map(Arc::from)
}

fn file_name(resource: &acp::EmbeddedResource) -> &str {
    let uri = match &resource.resource {
        acp::EmbeddedResourceResource::TextResourceContents(text) => text.uri.as_str(),
        acp::EmbeddedResourceResource::BlobResourceContents(blob) => blob.uri.as_str(),
        _ => "Attachment",
    };
    uri.rsplit('/').next().filter(|name| !name.is_empty()).unwrap_or(uri)
}
