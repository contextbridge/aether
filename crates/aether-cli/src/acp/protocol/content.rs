use agent_client_protocol::v2::schema as acp;
use foxfire_acp::content::map_content_blocks_to_text;
use llm::ContentBlock as LlmContentBlock;
use std::slice;

pub(crate) fn map_user_message(message_id: acp::MessageId, blocks: &[LlmContentBlock]) -> acp::UserMessage {
    acp::UserMessage::new(message_id).content(blocks.iter().map(map_user_content_block).collect::<Vec<_>>())
}

pub(crate) fn map_acp_to_content_blocks(blocks: Vec<acp::ContentBlock>) -> Vec<LlmContentBlock> {
    blocks
        .into_iter()
        .map(|block| match block {
            acp::ContentBlock::Text(text) => LlmContentBlock::text(text.text),
            acp::ContentBlock::Image(image) => {
                LlmContentBlock::Image { data: image.data, mime_type: image.mime_type.to_string() }
            }
            acp::ContentBlock::Audio(audio) => {
                LlmContentBlock::Audio { data: audio.data, mime_type: audio.mime_type.to_string() }
            }
            block => LlmContentBlock::text(map_content_blocks_to_text(slice::from_ref(&block))),
        })
        .collect()
}

pub(crate) fn map_user_content_block(block: &LlmContentBlock) -> acp::ContentBlock {
    match block {
        LlmContentBlock::Text { text } => acp::ContentBlock::from(text.clone()),
        LlmContentBlock::Image { data, mime_type } => {
            acp::ContentBlock::Image(acp::ImageContent::new(data.clone(), mime_type.clone()))
        }
        LlmContentBlock::Audio { data, mime_type } => {
            acp::ContentBlock::Audio(acp::AudioContent::new(data.clone(), mime_type.clone()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_conversion_preserves_media_and_shares_resource_display() {
        let media = vec![
            acp::ContentBlock::Image(acp::ImageContent::new("image-data", "image/png")),
            acp::ContentBlock::Audio(acp::AudioContent::new("audio-data", "audio/wav")),
        ];
        let converted = map_acp_to_content_blocks(media.clone());
        assert_eq!(converted.iter().map(map_user_content_block).collect::<Vec<_>>(), media);
        assert_eq!(map_content_blocks_to_text(&media), "[Image content]\n[Audio content]");
        let resources = vec![
            acp::ContentBlock::ResourceLink(acp::ResourceLink::new("readme", "file://readme")),
            acp::ContentBlock::Resource(acp::EmbeddedResource::new(
                acp::EmbeddedResourceResource::TextResourceContents(acp::TextResourceContents::new(
                    "contents",
                    "file://source",
                )),
            )),
        ];
        let converted = map_acp_to_content_blocks(resources.clone());
        let text = converted
            .iter()
            .map(|block| match block {
                LlmContentBlock::Text { text } => text.as_str(),
                _ => panic!("resources must become text"),
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(text, map_content_blocks_to_text(&resources));
    }
}
