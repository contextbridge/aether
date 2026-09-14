use agent_client_protocol::schema::v2 as acp;
use llm::ContentBlock as LlmContentBlock;

pub fn map_acp_to_content_blocks(blocks: Vec<acp::ContentBlock>) -> Vec<LlmContentBlock> {
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
            acp::ContentBlock::ResourceLink(link) => LlmContentBlock::text(format!("[Resource: {}]", link.uri)),
            acp::ContentBlock::Resource(resource) => LlmContentBlock::text(format_embedded_resource(&resource)),
            _ => LlmContentBlock::text("[Unknown content]"),
        })
        .collect()
}

pub fn map_user_content_block(block: &LlmContentBlock) -> acp::ContentBlock {
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

/// Converts ACP `ContentBlock` to plain text.
///
/// Embedded resources (e.g., file attachments) are formatted with their URI
/// and content for inclusion in the agent's context.
pub fn map_content_blocks_to_text(blocks: Vec<acp::ContentBlock>) -> String {
    map_acp_to_content_blocks(blocks)
        .into_iter()
        .map(|block| match block {
            LlmContentBlock::Text { text } => text,
            LlmContentBlock::Image { .. } => "[Image content]".to_string(),
            LlmContentBlock::Audio { .. } => "[Audio content]".to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Formats an embedded resource as text for inclusion in agent context.
pub fn format_embedded_resource(resource: &acp::EmbeddedResource) -> String {
    match &resource.resource {
        acp::EmbeddedResourceResource::TextResourceContents(text) => {
            format!("<file uri=\"{}\">\n{}\n</file>", text.uri, text.text)
        }
        acp::EmbeddedResourceResource::BlobResourceContents(blob) => {
            format!("[Binary resource: {}]", blob.uri)
        }
        _ => "[Unknown resource type]".to_string(),
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
        assert_eq!(map_content_blocks_to_text(media), "[Image content]\n[Audio content]");
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
        assert_eq!(text, map_content_blocks_to_text(resources));
    }

    #[test]
    fn test_format_embedded_resource_text() {
        let resource = acp::EmbeddedResource::new(acp::EmbeddedResourceResource::TextResourceContents(
            acp::TextResourceContents::new("let x = 1;", "file://test.rs"),
        ));

        let result = format_embedded_resource(&resource);

        assert_eq!(result, "<file uri=\"file://test.rs\">\nlet x = 1;\n</file>");
    }

    #[test]
    fn test_format_embedded_resource_blob() {
        let resource = acp::EmbeddedResource::new(acp::EmbeddedResourceResource::BlobResourceContents(
            acp::BlobResourceContents::new("base64data", "file://image.png"),
        ));

        let result = format_embedded_resource(&resource);

        assert_eq!(result, "[Binary resource: file://image.png]");
    }

    #[test]
    fn test_map_content_blocks_to_text_with_embedded_resource() {
        let blocks = vec![
            acp::ContentBlock::Text(acp::TextContent::new("Check this file:")),
            acp::ContentBlock::Resource(acp::EmbeddedResource::new(
                acp::EmbeddedResourceResource::TextResourceContents(
                    acp::TextResourceContents::new("pub fn hello() {}", "file://src/lib.rs").mime_type("text/x-rust"),
                ),
            )),
        ];

        let result = map_content_blocks_to_text(blocks);

        assert!(result.contains("Check this file:"));
        assert!(result.contains("<file uri=\"file://src/lib.rs\">"));
        assert!(result.contains("pub fn hello() {}"));
        assert!(result.contains("</file>"));
    }

    #[test]
    fn test_map_content_blocks_text_only() {
        let blocks = vec![
            acp::ContentBlock::Text(acp::TextContent::new("Hello")),
            acp::ContentBlock::Text(acp::TextContent::new("World")),
        ];

        assert_eq!(map_content_blocks_to_text(blocks), "Hello\nWorld");
    }

    #[test]
    fn test_map_content_blocks_empty() {
        assert_eq!(map_content_blocks_to_text(vec![]), "");
    }

    #[test]
    fn test_map_content_blocks_resource_link() {
        let blocks = vec![acp::ContentBlock::ResourceLink(acp::ResourceLink::new("readme.md", "file://readme.md"))];

        let result = map_content_blocks_to_text(blocks);
        assert_eq!(result, "[Resource: file://readme.md]");
    }
}
