use agent_client_protocol::schema as acp;

/// Converts ACP `ContentBlock` to plain text.
///
/// Embedded resources (e.g., file attachments) are formatted with their URI
/// and content for inclusion in the agent's context.
pub fn map_content_blocks_to_text(blocks: Vec<acp::ContentBlock>) -> String {
    blocks
        .into_iter()
        .map(|block| match block {
            acp::ContentBlock::Text(text) => text.text.clone(),
            acp::ContentBlock::Image(_) => "[Image content]".to_string(),
            acp::ContentBlock::Audio(_) => "[Audio content]".to_string(),
            acp::ContentBlock::ResourceLink(link) => {
                format!("[Resource: {}]", link.uri)
            }
            acp::ContentBlock::Resource(resource) => format_embedded_resource(&resource),
            _ => "[Unknown content]".to_string(),
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
