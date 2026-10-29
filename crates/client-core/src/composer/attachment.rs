use crate::session_model::WorkspaceAccess;
use agent_client_protocol::schema::v2 as acp;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachedFile {
    Media(PromptAttachment),
    Mention(PromptAttachment),
    Unsupported(PromptAttachment),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Text,
    Image,
    Audio,
    Unsupported,
}

#[derive(Debug, Default)]
pub struct AttachmentOutcome {
    pub blocks: Vec<acp::ContentBlock>,
    pub warnings: Vec<String>,
}

impl PromptAttachment {
    pub fn new(path: PathBuf, root: &Path) -> Self {
        let name = path.strip_prefix(root).ok().or_else(|| path.file_name().map(Path::new)).unwrap_or(&path);
        let display_name = name.to_string_lossy().replace('\\', "/");
        Self { path, display_name }
    }
}

pub fn classify_paths(paths: impl IntoIterator<Item = PathBuf>, root: &Path) -> Vec<AttachedFile> {
    paths
        .into_iter()
        .map(|path| {
            let kind = if path.is_file() { classify_attachment(&path) } else { AttachmentKind::Unsupported };
            let attachment = PromptAttachment::new(path, root);
            match kind {
                AttachmentKind::Image | AttachmentKind::Audio => AttachedFile::Media(attachment),
                AttachmentKind::Text => AttachedFile::Mention(attachment),
                AttachmentKind::Unsupported => AttachedFile::Unsupported(attachment),
            }
        })
        .collect()
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

pub fn build_attachments(attachments: &[PromptAttachment], access: WorkspaceAccess) -> AttachmentOutcome {
    let mut outcome = AttachmentOutcome::default();
    for attachment in attachments {
        let encoded = read_capped(&attachment.path, &attachment.display_name)
            .and_then(|bytes| encode_attachment(attachment, access, bytes));
        match encoded {
            Ok((block, warning)) => {
                outcome.blocks.push(block);
                outcome.warnings.extend(warning);
            }
            Err(warning) => outcome.warnings.push(warning),
        }
    }
    outcome
}

const MAX_EMBED_TEXT_BYTES: usize = 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 5 * 1024 * 1024;
const IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const AUDIO_MIME_TYPES: &[&str] = &["audio/wav", "audio/mpeg", "audio/mp3", "audio/ogg"];

/// Reads at most one byte past the largest embed cap, so truncation checks
/// never pull an unbounded file into memory.
fn read_capped(path: &Path, display_name: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?
        .take((MAX_MEDIA_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Failed to read {display_name}: {error}"))?;
    Ok(bytes)
}

/// Pure encoding of one attachment from its capped bytes.
fn encode_attachment(
    attachment: &PromptAttachment,
    access: WorkspaceAccess,
    bytes: Vec<u8>,
) -> Result<(acp::ContentBlock, Option<String>), String> {
    let PromptAttachment { path, display_name } = attachment;
    let mime_type = mime_guess::from_path(path).first_or_octet_stream().to_string();
    match classify_attachment(path) {
        AttachmentKind::Image | AttachmentKind::Audio => {
            encode_media_block(&bytes, display_name, &mime_type).map(|block| (block, None))
        }
        AttachmentKind::Text | AttachmentKind::Unsupported => {
            let uri = match access {
                WorkspaceAccess::Local => file_uri(path, display_name)?,
                WorkspaceAccess::Remote => attached_uri(display_name),
            };
            encode_text_block(bytes, uri, display_name, &mime_type)
        }
    }
}

fn encode_media_block(bytes: &[u8], display_name: &str, mime_type: &str) -> Result<acp::ContentBlock, String> {
    if bytes.len() > MAX_MEDIA_BYTES {
        return Err(format!("Skipped {display_name}: file too large (max {MAX_MEDIA_BYTES})"));
    }
    let data = BASE64.encode(bytes);
    Ok(if IMAGE_MIME_TYPES.contains(&mime_type) {
        acp::ContentBlock::Image(acp::ImageContent::new(data, mime_type))
    } else {
        acp::ContentBlock::Audio(acp::AudioContent::new(data, mime_type))
    })
}

fn encode_text_block(
    mut bytes: Vec<u8>,
    uri: String,
    display_name: &str,
    mime_type: &str,
) -> Result<(acp::ContentBlock, Option<String>), String> {
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

    let warning = truncated.then(|| format!("Truncated {display_name} to {MAX_EMBED_TEXT_BYTES} bytes"));
    Ok((
        acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new(text, uri).mime_type(mime_type),
        ))),
        warning,
    ))
}

fn file_uri(path: &Path, display_name: &str) -> Result<String, String> {
    let uri_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Url::from_file_path(uri_path)
        .map(|url| url.to_string())
        .map_err(|()| format!("Failed to build file URI for {display_name}"))
}

fn attached_uri(display_name: &str) -> String {
    let mut url = Url::parse("attachment:///").expect("a valid base URL");
    url.set_path(display_name);
    url.to_string()
}
