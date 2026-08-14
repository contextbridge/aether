use acp_utils::content::format_embedded_resource;
use agent_client_protocol::schema::v1::{self as acp, ContentBlock, TextContent};
use llm::ContentBlock as LlmContentBlock;

/// Convert client-supplied ACP content blocks into LLM content blocks for the
/// agent runtime. Unsupported/unknown blocks degrade to text.
pub(crate) fn map_acp_to_content_blocks(blocks: Vec<ContentBlock>) -> Vec<LlmContentBlock> {
    blocks
        .into_iter()
        .map(|block| match block {
            ContentBlock::Text(t) => LlmContentBlock::text(t.text),
            ContentBlock::Image(img) => LlmContentBlock::Image { data: img.data, mime_type: img.mime_type },
            ContentBlock::Audio(aud) => LlmContentBlock::Audio { data: aud.data, mime_type: aud.mime_type },
            ContentBlock::Resource(r) => LlmContentBlock::text(format_embedded_resource(&r)),
            ContentBlock::ResourceLink(l) => LlmContentBlock::text(format!("[Resource: {}]", l.uri)),
            _ => LlmContentBlock::text("[Unknown content]"),
        })
        .collect()
}

/// Convert a stored LLM content block back into an ACP content block for replay.
pub(crate) fn map_user_content_block(block: &LlmContentBlock) -> ContentBlock {
    match block {
        LlmContentBlock::Text { text } => ContentBlock::Text(TextContent::new(text.clone())),
        LlmContentBlock::Image { data, mime_type } => {
            ContentBlock::Image(acp::ImageContent::new(data.clone(), mime_type.clone()))
        }
        LlmContentBlock::Audio { data, mime_type } => {
            ContentBlock::Audio(acp::AudioContent::new(data.clone(), mime_type.clone()))
        }
    }
}
