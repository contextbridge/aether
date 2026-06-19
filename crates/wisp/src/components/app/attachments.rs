use super::PromptAttachment;
use agent_client_protocol::schema as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::path::Path;
use tokio::io::AsyncReadExt;
use url::Url;

const MAX_EMBED_TEXT_BYTES: usize = 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 10 * 1024 * 1024;

const IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp"];
const AUDIO_MIME_TYPES: &[&str] = &["audio/wav", "audio/mpeg", "audio/mp3", "audio/ogg"];

#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    #[error("Failed to read {name}: {source}")]
    Read {
        name: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Skipped {name}: file too large ({size} bytes, max {max})")]
    TooLarge { name: String, size: u64, max: usize },
    #[error("Skipped binary or non-UTF8 file: {0}")]
    NotUtf8(String),
    #[error("Failed to build file URI for {0}")]
    FileUri(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentKind {
    Text,
    Image,
    Audio,
    Unsupported,
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

#[derive(Debug, Default)]
pub struct AttachmentBuildOutcome {
    pub blocks: Vec<acp::ContentBlock>,
    pub transcript_placeholders: Vec<String>,
    pub warnings: Vec<String>,
}

pub async fn build_attachment_blocks(attachments: &[PromptAttachment]) -> AttachmentBuildOutcome {
    let mut outcome = AttachmentBuildOutcome::default();

    for attachment in attachments {
        match try_build_attachment_block(&attachment.path, &attachment.display_name).await {
            Ok(result) => {
                outcome.blocks.push(result.block);
                if let Some(placeholder) = result.transcript_placeholder {
                    outcome.transcript_placeholders.push(placeholder);
                }
                if let Some(warning) = result.warning {
                    outcome.warnings.push(warning);
                }
            }
            Err(warning) => outcome.warnings.push(warning.to_string()),
        }
    }

    outcome
}

struct AttachmentBlockResult {
    block: acp::ContentBlock,
    transcript_placeholder: Option<String>,
    warning: Option<String>,
}

async fn try_build_attachment_block(path: &Path, display_name: &str) -> Result<AttachmentBlockResult, AttachmentError> {
    let kind = classify_attachment(path);
    let mime_type = mime_guess::from_path(path).first_or_octet_stream().to_string();

    match kind {
        AttachmentKind::Image | AttachmentKind::Audio => {
            let bytes = read_media_bytes(path, display_name).await?;
            let data = BASE64.encode(&bytes);
            let (block, placeholder) = match kind {
                AttachmentKind::Image => (
                    acp::ContentBlock::Image(acp::ImageContent::new(data, &mime_type)),
                    format!("[image attachment: {display_name}]"),
                ),
                _ => (
                    acp::ContentBlock::Audio(acp::AudioContent::new(data, &mime_type)),
                    format!("[audio attachment: {display_name}]"),
                ),
            };
            Ok(AttachmentBlockResult { block, transcript_placeholder: Some(placeholder), warning: None })
        }
        _ => build_text_resource_block(path, display_name, &mime_type).await,
    }
}

async fn read_media_bytes(path: &Path, display_name: &str) -> Result<Vec<u8>, AttachmentError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|e| AttachmentError::Read { name: display_name.to_string(), source: e })?;

    if metadata.len() > MAX_MEDIA_BYTES as u64 {
        return Err(AttachmentError::TooLarge {
            name: display_name.to_string(),
            size: metadata.len(),
            max: MAX_MEDIA_BYTES,
        });
    }

    tokio::fs::read(path).await.map_err(|e| AttachmentError::Read { name: display_name.to_string(), source: e })
}

async fn build_text_resource_block(
    path: &Path,
    display_name: &str,
    mime_type: &str,
) -> Result<AttachmentBlockResult, AttachmentError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|e| AttachmentError::Read { name: display_name.to_string(), source: e })?;

    let mut bytes = Vec::new();
    file.take((MAX_EMBED_TEXT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .await
        .map_err(|e| AttachmentError::Read { name: display_name.to_string(), source: e })?;

    let truncated = bytes.len() > MAX_EMBED_TEXT_BYTES;
    if truncated {
        bytes.truncate(MAX_EMBED_TEXT_BYTES);
    }
    let text_bytes = bytes.as_slice();

    let text = match std::str::from_utf8(text_bytes) {
        Ok(text) => text.to_string(),
        Err(error) if truncated && error.valid_up_to() > 0 => {
            let valid_bytes = &text_bytes[..error.valid_up_to()];
            std::str::from_utf8(valid_bytes).expect("valid_up_to must point at a utf8 boundary").to_string()
        }
        Err(_) => return Err(AttachmentError::NotUtf8(display_name.to_string())),
    };

    let file_uri = build_attachment_file_uri(path, display_name).await?;
    let warning = truncated.then(|| format!("Truncated {display_name} to {MAX_EMBED_TEXT_BYTES} bytes"));

    let block =
        acp::ContentBlock::Resource(acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new(text, file_uri).mime_type(mime_type),
        )));

    Ok(AttachmentBlockResult { block, transcript_placeholder: None, warning })
}

async fn build_attachment_file_uri(path: &Path, display_name: &str) -> Result<String, AttachmentError> {
    let canonical_path = tokio::fs::canonicalize(path).await.ok();
    let uri_path = canonical_path.as_deref().unwrap_or(path);
    Url::from_file_path(uri_path)
        .map_err(|()| AttachmentError::FileUri(display_name.to_string()))
        .map(|url| url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn build_attachment_blocks_truncates_large_file_with_warning() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("large.txt");
        let display_name = "large.txt".to_string();
        std::fs::write(&path, "x".repeat(MAX_EMBED_TEXT_BYTES + 64)).unwrap();

        let attachments = vec![PromptAttachment { path, display_name: display_name.clone() }];
        let outcome = build_attachment_blocks(&attachments).await;

        assert_eq!(outcome.blocks.len(), 1);
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains(&format!("Truncated {display_name} to {MAX_EMBED_TEXT_BYTES} bytes")));
    }

    #[tokio::test]
    async fn build_attachment_blocks_skips_non_utf8_files() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("binary.bin");
        let display_name = "binary.bin".to_string();
        std::fs::write(&path, [0xff, 0xfe, 0xfd]).unwrap();

        let attachments = vec![PromptAttachment { path, display_name: display_name.clone() }];
        let outcome = build_attachment_blocks(&attachments).await;

        assert!(outcome.blocks.is_empty());
        assert_eq!(outcome.warnings.len(), 1);
        assert!(outcome.warnings[0].contains(&format!("Skipped binary or non-UTF8 file: {display_name}")));
    }

    #[tokio::test]
    async fn build_attachment_file_uri_falls_back_when_canonicalize_fails() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("missing.txt");
        let expected = Url::from_file_path(&path).unwrap().to_string();

        let uri = build_attachment_file_uri(&path, "missing.txt")
            .await
            .expect("URI should be built from original absolute path");

        assert_eq!(uri, expected);
    }

    #[tokio::test]
    async fn png_file_produces_image_content_block() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.png");
        std::fs::write(&path, b"fake png data").unwrap();

        let attachments = vec![PromptAttachment { path, display_name: "test.png".to_string() }];
        let outcome = build_attachment_blocks(&attachments).await;

        assert_eq!(outcome.blocks.len(), 1);
        assert!(outcome.warnings.is_empty());
        assert_eq!(outcome.transcript_placeholders, vec!["[image attachment: test.png]"]);
        assert!(matches!(outcome.blocks[0], acp::ContentBlock::Image(_)));
    }

    #[tokio::test]
    async fn wav_file_produces_audio_content_block() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.wav");
        std::fs::write(&path, b"fake wav data").unwrap();

        let attachments = vec![PromptAttachment { path, display_name: "test.wav".to_string() }];
        let outcome = build_attachment_blocks(&attachments).await;

        assert_eq!(outcome.blocks.len(), 1);
        assert!(outcome.warnings.is_empty());
        assert_eq!(outcome.transcript_placeholders, vec!["[audio attachment: test.wav]"]);
        assert!(matches!(outcome.blocks[0], acp::ContentBlock::Audio(_)));
    }

    #[test]
    fn classify_attachment_detects_images() {
        assert_eq!(classify_attachment(Path::new("photo.png")), AttachmentKind::Image);
        assert_eq!(classify_attachment(Path::new("photo.jpg")), AttachmentKind::Image);
        assert_eq!(classify_attachment(Path::new("photo.gif")), AttachmentKind::Image);
        assert_eq!(classify_attachment(Path::new("photo.webp")), AttachmentKind::Image);
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
}
