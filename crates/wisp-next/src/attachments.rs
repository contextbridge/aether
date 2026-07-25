use agent_client_protocol::schema as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::path::{Path, PathBuf};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAttachment {
    pub path: PathBuf,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Text,
    Image,
    Audio,
    Unsupported,
}

pub struct AttachmentOutcome {
    pub blocks: Vec<acp::ContentBlock>,
    pub placeholders: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn classify_attachment(path: &Path) -> AttachmentKind {
    let mime = mime_guess::from_path(path).first_or_octet_stream().to_string();

    if mime.starts_with("image/") {
        AttachmentKind::Image
    } else if mime.starts_with("audio/") {
        AttachmentKind::Audio
    } else if mime.starts_with("text/") {
        AttachmentKind::Text
    } else {
        AttachmentKind::Unsupported
    }
}

pub fn build_attachments(attachments: &[PromptAttachment]) -> AttachmentOutcome {
    let mut outcome = AttachmentOutcome { blocks: Vec::new(), placeholders: Vec::new(), warnings: Vec::new() };
    for attachment in attachments {
        match build_one(&attachment.path, &attachment.display_name) {
            Ok((block, placeholder)) => {
                outcome.blocks.push(block);
                if let Some(placeholder) = placeholder {
                    outcome.placeholders.push(placeholder);
                }
            }
            Err(warning) => outcome.warnings.push(warning),
        }
    }
    outcome
}

const MAX_ATTACHMENT_BYTES: usize = 1_000_000;

fn build_one(path: &Path, display_name: &str) -> Result<(acp::ContentBlock, Option<String>), String> {
    let bytes = std::fs::read(path).map_err(|error| format!("Could not attach {display_name}: {error}"))?;
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        return Err(format!("Skipped {display_name}: attachment exceeds {MAX_ATTACHMENT_BYTES} bytes"));
    }
    let mime_type = mime_guess::from_path(path).first_or_octet_stream().to_string();
    if mime_type.starts_with("image/") {
        return Ok((
            acp::ContentBlock::Image(acp::ImageContent::new(BASE64.encode(bytes), &mime_type)),
            Some(format!("[image attachment: {display_name}]")),
        ));
    }
    if mime_type.starts_with("audio/") {
        return Ok((
            acp::ContentBlock::Audio(acp::AudioContent::new(BASE64.encode(bytes), &mime_type)),
            Some(format!("[audio attachment: {display_name}]")),
        ));
    }
    let text = String::from_utf8(bytes).map_err(|_| format!("Skipped binary or non-UTF-8 file: {display_name}"))?;
    let uri_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let uri = Url::from_file_path(uri_path)
        .map_err(|()| format!("Could not create a file URI for {display_name}"))?
        .to_string();
    Ok((
        acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new(text, uri).mime_type(mime_type),
        ))),
        None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_attachment_detects_images() {
        assert_eq!(classify_attachment(Path::new("photo.png")), AttachmentKind::Image);
        assert_eq!(classify_attachment(Path::new("photo.jpg")), AttachmentKind::Image);
        assert_eq!(classify_attachment(Path::new("photo.gif")), AttachmentKind::Image);
        assert_eq!(classify_attachment(Path::new("photo.bmp")), AttachmentKind::Image);
    }

    #[test]
    fn classify_attachment_detects_audio() {
        assert_eq!(classify_attachment(Path::new("note.wav")), AttachmentKind::Audio);
        assert_eq!(classify_attachment(Path::new("note.mp3")), AttachmentKind::Audio);
        assert_eq!(classify_attachment(Path::new("note.ogg")), AttachmentKind::Audio);
    }

    #[test]
    fn classify_attachment_detects_text() {
        assert_eq!(classify_attachment(Path::new("readme.txt")), AttachmentKind::Text);
    }

    #[test]
    fn classify_attachment_unknown_extension_is_unsupported() {
        assert_eq!(classify_attachment(Path::new("data.xyz")), AttachmentKind::Unsupported);
    }

    #[test]
    fn build_from_attachment_produces_image_block() {
        use tempfile::TempDir;
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.png");
        std::fs::write(&path, b"fake png data").unwrap();

        let attachments = vec![PromptAttachment { path, display_name: "test.png".to_string() }];
        let outcome = build_attachments(&attachments);

        assert_eq!(outcome.blocks.len(), 1);
        assert!(outcome.warnings.is_empty());
        assert_eq!(outcome.placeholders, vec!["[image attachment: test.png]"]);
        assert!(matches!(outcome.blocks[0], acp::ContentBlock::Image(_)));
    }
}
