use crate::composer::SelectedFileMention;
use agent_client_protocol::schema as acp;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use std::path::Path;
use url::Url;

pub struct AttachmentOutcome {
    pub blocks: Vec<acp::ContentBlock>,
    pub placeholders: Vec<String>,
    pub warnings: Vec<String>,
}

pub fn build(mentions: &[SelectedFileMention]) -> AttachmentOutcome {
    let mut outcome = AttachmentOutcome { blocks: Vec::new(), placeholders: Vec::new(), warnings: Vec::new() };
    for mention in mentions {
        match build_one(&mention.path, &mention.display_name) {
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
