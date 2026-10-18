use super::McpCatalog;
use crate::events::{TaskOutcome, TaskOutcomeState};
use mcp_utils::client::{ToolCall, ToolCallError as McpToolCallError, ToolCallOptions};
use rmcp::model::{CallToolResult, ContentBlock, EmbeddedResource, ResourceContents, Tool};
use serde_json::Value;
use thiserror::Error;

use llm::{ToolAnnotations, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use utils::display_meta::ToolResultMeta;
use utils::temp_dir::TempDir;
use utils::tool_result_truncator::ToolResultTruncator;

const TOOL_RESULT_TRUNCATOR: ToolResultTruncator = ToolResultTruncator { head: 25_000, tail: 25_000 };

#[derive(Debug, Error)]
pub(crate) enum ToolRejection {
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Invalid tool arguments: {0}")]
    InvalidArguments(#[source] serde_json::Error),
}

pub(crate) fn convert_tool_result(
    request: &ToolCallRequest,
    outcome: Result<CallToolResult, McpToolCallError>,
    spill_dir: &TempDir,
) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolCallError> {
    let mcp_result = outcome.map_err(|error| ToolCallError::from_request(request, error.to_string()))?;
    if mcp_result.is_error == Some(true) {
        let text = content_text(&mcp_result.content, "Unknown error");
        let message = TOOL_RESULT_TRUNCATOR.truncate(text, spill_dir, &request.name);
        return Err(ToolCallError::from_request(request, format!("Tool execution error: {message}")));
    }

    let result_meta = mcp_result.meta.and_then(|meta| serde_json::from_value(Value::Object(meta.0)).ok());
    let result = match mcp_result.structured_content {
        Some(value) => encode_structured(&value),
        None => content_text(&mcp_result.content, "No result"),
    };
    let result = TOOL_RESULT_TRUNCATOR.truncate(result, spill_dir, &request.name);

    Ok((
        ToolCallResult {
            id: request.id.clone(),
            name: request.name.clone(),
            arguments: request.arguments.clone(),
            result,
        },
        result_meta,
    ))
}

pub(crate) fn map_task_result_to_outcome(
    request: ToolCallRequest,
    task_id: String,
    outcome: Result<CallToolResult, McpToolCallError>,
    spill_dir: &TempDir,
) -> TaskOutcome {
    let state = match outcome {
        Err(McpToolCallError::Cancelled) => TaskOutcomeState::Cancelled,
        outcome => match convert_tool_result(&request, outcome, spill_dir) {
            Ok((result, result_meta)) => TaskOutcomeState::Completed { result, result_meta },
            Err(error) => TaskOutcomeState::Failed { error },
        },
    };
    TaskOutcome { request, task_id, state }
}

pub(crate) fn call_tool(
    mcp: &McpCatalog,
    request: &ToolCallRequest,
    options: ToolCallOptions,
) -> Result<ToolCall, ToolRejection> {
    let arguments = serde_json::from_str(&request.arguments).map_err(ToolRejection::InvalidArguments)?;
    mcp.call_tool(&request.name, arguments, options).ok_or_else(|| ToolRejection::NotFound(request.name.clone()))
}

pub(crate) fn encode_structured(value: &serde_json::Value) -> String {
    noyalib::to_string(value).unwrap_or_else(|_| value.to_string())
}

pub(super) fn tool_definition(tool: &Tool) -> ToolDefinition {
    let annotations = tool.annotations.as_ref().map(|annotations| ToolAnnotations {
        title: annotations.title.clone(),
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    });
    ToolDefinition::new(
        tool.name.to_string(),
        tool.description.clone().unwrap_or_default(),
        Value::Object((*tool.input_schema).clone()),
    )
    .with_annotations(annotations)
}

fn content_text(content: &[ContentBlock], empty: &str) -> String {
    if content.is_empty() {
        return empty.to_string();
    }
    content.iter().map(block_text).collect::<Vec<_>>().join("\n")
}

fn block_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Image(image) => binary_placeholder(&image.mime_type, &image.data),
        ContentBlock::Audio(audio) => binary_placeholder(&audio.mime_type, &audio.data),
        ContentBlock::Resource(EmbeddedResource {
            resource: ResourceContents::TextResourceContents { text, .. },
            ..
        }) => text.clone(),
        ContentBlock::Resource(EmbeddedResource {
            resource: ResourceContents::BlobResourceContents { mime_type, blob, .. },
            ..
        }) => binary_placeholder(mime_type.as_deref().unwrap_or("binary"), blob),
        block => serde_json::to_string(block).unwrap_or_default(),
    }
}

fn binary_placeholder(mime_type: &str, base64: &str) -> String {
    format!("[{mime_type} content omitted, {} bytes of base64]", base64.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolResult as McpCallToolResult;
    use serde_json::json;
    use utils::display_meta::PlanMetaStatus;

    fn req() -> ToolCallRequest {
        ToolCallRequest { id: "call_123".into(), name: "test_tool".into(), arguments: "{}".into() }
    }

    fn convert(mcp: McpCallToolResult) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolCallError> {
        convert_tool_result(&req(), Ok::<_, McpToolCallError>(mcp), &TempDir::new())
    }

    fn call_structured(structured: serde_json::Value) -> (ToolCallResult, Option<ToolResultMeta>) {
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.content = vec![];
        convert(mcp).unwrap()
    }

    fn with_meta(structured: serde_json::Value, meta: serde_json::Value) -> McpCallToolResult {
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.meta = Some(serde_json::from_value(meta).unwrap());
        mcp
    }

    #[test]
    fn display_metadata_comes_from_the_result_meta() {
        let (result, meta) = convert(with_meta(
            json!({ "status": "success", "file_path": "/test/file.rs" }),
            json!({
                "display": { "title": "Edit file", "value": "main.rs" },
                "file_diff": { "path": "/tmp/main.rs", "old_text": "old content", "new_text": "new content" },
                "plan": { "entries": [
                    { "content": "Research AI agents", "status": "in_progress" },
                    { "content": "Write tests", "status": "pending" }
                ]},
                "traceparent": "00-trace-span-01"
            }),
        ))
        .unwrap();

        assert_eq!(result.result, encode_structured(&json!({ "status": "success", "file_path": "/test/file.rs" })));
        let meta = meta.expect("meta should be present");
        assert_eq!(meta.display.title, "Edit file");
        assert_eq!(meta.display.value, "main.rs");
        let diff = meta.file_diff.expect("file_diff should be present");
        assert_eq!(diff.path, "/tmp/main.rs");
        assert_eq!(diff.old_text.as_deref(), Some("old content"));
        assert_eq!(diff.new_text.as_deref(), Some("new content"));
        let plan = meta.plan.expect("plan should be present");
        assert_eq!(plan.entries[0].status, PlanMetaStatus::InProgress);
        assert_eq!(plan.entries[1].status, PlanMetaStatus::Pending);
    }

    #[test]
    fn result_meta_without_display_metadata_is_ignored() {
        for meta in [json!({ "display": "not a valid ToolDisplayMeta" }), json!({ "traceparent": "00-trace-span-01" })]
        {
            let (result, display) = convert(with_meta(json!({ "status": "success" }), meta)).unwrap();
            assert!(display.is_none());
            assert!(result.result.contains("success"));
        }
    }

    #[test]
    fn test_no_meta_passes_through_unchanged() {
        let (result, meta) = call_structured(json!({"status": "success", "data": "hello"}));
        assert!(result.result.contains("success"));
        assert!(result.result.contains("hello"));
        assert!(meta.is_none());
    }

    #[test]
    fn text_results_are_returned_as_plain_text() {
        let mcp = McpCallToolResult::success(vec![ContentBlock::text("plain text result")]);
        let (result, meta) = convert(mcp).unwrap();
        assert_eq!(result.result, "plain text result");
        assert!(meta.is_none());
    }

    #[test]
    fn text_results_join_every_content_block() {
        let mcp = McpCallToolResult::success(vec![ContentBlock::text("first"), ContentBlock::text("second")]);
        let (result, _) = convert(mcp).unwrap();
        assert_eq!(result.result, "first\nsecond");
    }

    #[test]
    fn test_tool_call_result_handles_text_error_without_sdk_debug_output() {
        let mcp = McpCallToolResult::error(vec![ContentBlock::text("Error: file not found")]);
        let err = convert(mcp).unwrap_err();
        assert_eq!(err.error, "Tool execution error: Error: file not found");
    }

    #[test]
    fn binary_content_is_summarized_instead_of_inlined() {
        let image = ContentBlock::image("aW1hZ2U=", "image/png");
        let mcp = McpCallToolResult::success(vec![ContentBlock::text("Captured screenshot"), image.clone()]);
        let (result, _) = convert(mcp).unwrap();
        assert_eq!(result.result, "Captured screenshot\n[image/png content omitted, 8 bytes of base64]");

        let err = convert(McpCallToolResult::error(vec![image])).unwrap_err();
        assert_eq!(err.error, "Tool execution error: [image/png content omitted, 8 bytes of base64]");
    }

    #[test]
    fn embedded_text_resources_are_returned_as_their_text() {
        let mcp = McpCallToolResult::success(vec![ContentBlock::embedded_text("file:///a.rs", "fn main() {\n}")]);
        let (result, _) = convert(mcp).unwrap();
        assert_eq!(result.result, "fn main() {\n}");
    }

    #[test]
    fn non_binary_content_is_serialized() {
        let link =
            serde_json::from_value(json!({"type": "resource_link", "uri": "file:///a.rs", "name": "a.rs"})).unwrap();
        let err = convert(McpCallToolResult::error(vec![link])).unwrap_err();
        assert_eq!(err.error, r#"Tool execution error: {"type":"resource_link","uri":"file:///a.rs","name":"a.rs"}"#);
    }

    #[test]
    fn test_result_is_yaml_format() {
        let (result, _) = call_structured(json!({
            "status": "success",
            "files": [{"name": "Cargo.toml", "path": "./Cargo.toml"}, {"name": "src", "path": "./src"}],
            "totalCount": 2
        }));
        let r = &result.result;
        for expected in ["status: success", "totalCount: 2", "- name:"] {
            assert!(r.contains(expected), "expected '{expected}' in YAML, got: {r}");
        }
        assert!(!r.starts_with('{'), "expected YAML, not JSON: {r}");
    }

    #[test]
    fn structured_results_yaml_cannot_hold_fall_back_to_json() {
        let (result, _) = call_structured(json!({"id": u64::MAX}));
        assert_eq!(result.result, r#"{"id":18446744073709551615}"#);
    }

    #[test]
    fn oversized_results_and_errors_keep_their_head_and_tail_and_save_the_full_text() {
        let text = (1..=100_000).map(|n| format!("{n}\n")).collect::<Vec<_>>().concat();
        let spill_dir = TempDir::new();
        let convert = |mcp| convert_tool_result(&req(), Ok::<_, McpToolCallError>(mcp), &spill_dir);
        let (result, _) = convert(McpCallToolResult::success(vec![ContentBlock::text(&text)])).unwrap();
        let error = convert(McpCallToolResult::error(vec![ContentBlock::text(&text)])).unwrap_err().error;

        for excerpt in [result.result.as_str(), error.strip_prefix("Tool execution error: ").unwrap()] {
            assert!(excerpt.starts_with("1\n2\n3\n"), "{excerpt}");
            assert!(excerpt.ends_with("99999\n100000\n"), "{excerpt}");
            assert!(excerpt.len() < text.len() / 5, "{excerpt}");
            let saved = utils::tool_result_truncator::saved_path(excerpt).expect("excerpt names its saved file");
            assert_eq!(std::fs::read_to_string(saved).unwrap(), text);
        }
    }
}
