use agent_client_protocol::schema as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::io::Read;
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

impl AttachmentOutcome {
    pub fn failed(warning: String) -> Self {
        Self { blocks: Vec::new(), placeholders: Vec::new(), warnings: vec![warning] }
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
    let mut outcome = AttachmentOutcome { blocks: Vec::new(), placeholders: Vec::new(), warnings: Vec::new() };
    for attachment in attachments {
        match build_one(&attachment.path, &attachment.display_name) {
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

const MAX_EMBED_TEXT_BYTES: usize = 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 10 * 1024 * 1024;
const IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const AUDIO_MIME_TYPES: &[&str] = &["audio/wav", "audio/mpeg", "audio/mp3", "audio/ogg"];

fn build_one(path: &Path, display_name: &str) -> Result<(acp::ContentBlock, Option<String>, Option<String>), String> {
    let mime_type = mime_guess::from_path(path).first_or_octet_stream().to_string();
    match classify_attachment(path) {
        AttachmentKind::Image | AttachmentKind::Audio => {
            build_media_block(path, display_name, &mime_type).map(|(block, placeholder)| (block, placeholder, None))
        }
        AttachmentKind::Text | AttachmentKind::Unsupported => build_text_block(path, display_name, &mime_type),
    }
}

fn build_media_block(
    path: &Path,
    display_name: &str,
    mime_type: &str,
) -> Result<(acp::ContentBlock, Option<String>), String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("Failed to read {display_name}: {error}"))?;
    if metadata.len() > MAX_MEDIA_BYTES as u64 {
        return Err(format!(
            "Skipped {display_name}: file too large ({size} bytes, max {MAX_MEDIA_BYTES})",
            size = metadata.len()
        ));
    }
    // metadata().len() is a point-in-time snapshot; cap the read so a file that grows
    // between the size check and the read cannot force an unbounded allocation.
    let capacity = metadata.len().min(MAX_MEDIA_BYTES as u64);
    let mut bytes = Vec::with_capacity(usize::try_from(capacity).unwrap_or(MAX_MEDIA_BYTES));
    std::fs::File::open(path)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?
        .take((MAX_MEDIA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?;
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err(format!("Skipped {display_name}: file too large (max {MAX_MEDIA_BYTES})"));
    }
    let data = BASE64.encode(&bytes);
    let (block, placeholder) = if IMAGE_MIME_TYPES.contains(&mime_type) {
        (
            acp::ContentBlock::Image(acp::ImageContent::new(data, mime_type)),
            format!("[image attachment: {display_name}]"),
        )
    } else {
        (
            acp::ContentBlock::Audio(acp::AudioContent::new(data, mime_type)),
            format!("[audio attachment: {display_name}]"),
        )
    };
    Ok((block, Some(placeholder)))
}

fn build_text_block(
    path: &Path,
    display_name: &str,
    mime_type: &str,
) -> Result<(acp::ContentBlock, Option<String>, Option<String>), String> {
    let file = std::fs::File::open(path).map_err(|error| format!("Failed to read {display_name}: {error}"))?;
    // Read at most one byte past the cap so truncating never pulls an unbounded file into memory.
    let mut bytes = Vec::new();
    file.take((MAX_EMBED_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?;

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
