use agent_client_protocol::schema::v1 as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::io::Read;
use std::path::{Path, PathBuf};
use url::Url;

const IMAGE_ATTACHMENT_LABEL: &str = "image attachment";
const AUDIO_ATTACHMENT_LABEL: &str = "audio attachment";
pub(crate) const IMAGE_ATTACHMENT_PLACEHOLDER: &str = "[image attachment]";
pub(crate) const AUDIO_ATTACHMENT_PLACEHOLDER: &str = "[audio attachment]";

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

pub(crate) fn placeholder_for_content_block(block: &acp::ContentBlock) -> Option<&'static str> {
    match block {
        acp::ContentBlock::Image(_) => Some(IMAGE_ATTACHMENT_PLACEHOLDER),
        acp::ContentBlock::Audio(_) => Some(AUDIO_ATTACHMENT_PLACEHOLDER),
        _ => None,
    }
}

pub fn classify_attachment(path: &Path) -> AttachmentKind {
    let mime = mime_guess::from_path(path).first_or_octet_stream().to_string();
    if IMAGE_MIME_TYPES.contains(&mime.as_str()) {
        AttachmentKind::Image
    } else if AUDIO_MIME_TYPES.contains(&mime.as_str()) {
        AttachmentKind::Audio
    } else if mime.starts_with("text/") {
        AttachmentKind::Text
    } else {
        AttachmentKind::Unsupported
    }
}

pub fn build_attachments(attachments: &[PromptAttachment]) -> AttachmentOutcome {
    build_attachments_with(attachments, read_capped)
}

/// The accumulation shared by the real reader and the in-memory test
/// filesystem: `read` performs the only side effect, everything after the
/// bytes is pure encoding.
pub(crate) fn build_attachments_with(
    attachments: &[PromptAttachment],
    mut read: impl FnMut(&Path, &str) -> Result<Vec<u8>, String>,
) -> AttachmentOutcome {
    let mut outcome = AttachmentOutcome { blocks: Vec::new(), placeholders: Vec::new(), warnings: Vec::new() };
    for attachment in attachments {
        let encoded = read(&attachment.path, &attachment.display_name)
            .and_then(|bytes| encode_attachment(&attachment.path, &attachment.display_name, bytes));
        match encoded {
            Ok((block, placeholder, warning)) => {
                outcome.blocks.push(block);
                if let Some(placeholder) = placeholder {
                    outcome.placeholders.push(placeholder);
                }
                if let Some(warning) = warning {
                    outcome.warnings.push(warning);
                }
            }
            Err(warning) => outcome.warnings.push(warning),
        }
    }
    outcome
}

/// Reads at most one byte past the largest embed cap, so truncation checks
/// never pull an unbounded file into memory.
pub(crate) fn read_capped(path: &Path, display_name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?
        .take((MAX_MEDIA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?;
    Ok(bytes)
}

const MAX_EMBED_TEXT_BYTES: usize = 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 10 * 1024 * 1024;
const IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const AUDIO_MIME_TYPES: &[&str] = &["audio/wav", "audio/mpeg", "audio/mp3", "audio/ogg"];

/// Pure encoding of one attachment from its capped bytes.
pub(crate) fn encode_attachment(
    path: &Path,
    display_name: &str,
    bytes: Vec<u8>,
) -> Result<(acp::ContentBlock, Option<String>, Option<String>), String> {
    let mime_type = mime_guess::from_path(path).first_or_octet_stream().to_string();
    match classify_attachment(path) {
        AttachmentKind::Image | AttachmentKind::Audio => {
            encode_media_block(&bytes, display_name, &mime_type).map(|(block, placeholder)| (block, placeholder, None))
        }
        AttachmentKind::Text | AttachmentKind::Unsupported => encode_text_block(bytes, path, display_name, &mime_type),
    }
}

fn encode_media_block(
    bytes: &[u8],
    display_name: &str,
    mime_type: &str,
) -> Result<(acp::ContentBlock, Option<String>), String> {
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err(format!("Skipped {display_name}: file too large (max {MAX_MEDIA_BYTES})"));
    }
    let data = BASE64.encode(bytes);
    let (block, placeholder) = if IMAGE_MIME_TYPES.contains(&mime_type) {
        (
            acp::ContentBlock::Image(acp::ImageContent::new(data, mime_type)),
            format!("[{IMAGE_ATTACHMENT_LABEL}: {display_name}]"),
        )
    } else {
        (
            acp::ContentBlock::Audio(acp::AudioContent::new(data, mime_type)),
            format!("[{AUDIO_ATTACHMENT_LABEL}: {display_name}]"),
        )
    };
    Ok((block, Some(placeholder)))
}

fn encode_text_block(
    mut bytes: Vec<u8>,
    path: &Path,
    display_name: &str,
    mime_type: &str,
) -> Result<(acp::ContentBlock, Option<String>, Option<String>), String> {
    let truncated = bytes.len() > MAX_EMBED_TEXT_BYTES;
    if truncated {
        bytes.truncate(MAX_EMBED_TEXT_BYTES);
    }

    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text.to_string(),
        // Truncation can only split the final multi-byte code point, so back up to the
        // last complete boundary. A definite invalid sequence (error_len) means the file
        // is genuinely non-UTF-8 and must be rejected rather than silently truncated.
        Err(error) if truncated && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid_up_to marks a UTF-8 boundary").to_string()
        }
        Err(_) => return Err(format!("Skipped binary or non-UTF8 file: {display_name}")),
    };

    let uri = attachment_uri(path, display_name)?;
    let warning = truncated.then(|| format!("Truncated {display_name} to {MAX_EMBED_TEXT_BYTES} bytes"));
    Ok((
        acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new(text, uri).mime_type(mime_type),
        ))),
        None,
        warning,
    ))
}

fn attachment_uri(path: &Path, display_name: &str) -> Result<String, String> {
    let uri_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Url::from_file_path(uri_path)
        .map(|url| url.to_string())
        .map_err(|()| format!("Failed to build file URI for {display_name}"))
}
