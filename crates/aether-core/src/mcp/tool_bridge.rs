use crate::events::{TaskOutcome, TaskOutcomeState};
use mcp_utils::McpError;
use mcp_utils::client::{ToolCall, ToolCallError as McpToolCallError, ToolCallOptions};
use mcp_utils::gateway::{McpCatalog, McpGateway, split_namespaced};
use rmcp::model::{CallToolResult, ContentBlock, EmbeddedResource, ResourceContents, Tool};
use serde_json::{Map, Value};

use llm::{ToolAnnotations, ToolCallError, ToolCallRequest, ToolCallResult, ToolDefinition};
use utils::display_meta::ToolResultMeta;
use utils::temp_dir::TempDir;
use utils::tool_result_truncator::ToolResultTruncator;

const TOOL_RESULT_TRUNCATOR: ToolResultTruncator = ToolResultTruncator { head: 25_000, tail: 25_000 };

pub fn tool_definitions(catalog: &McpCatalog) -> Vec<ToolDefinition> {
    catalog.tools().iter().map(tool_definition).collect()
}

pub fn convert_tool_result(
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

    let (result, result_meta) = match mcp_result.structured_content {
        Some(mut value) => {
            let result_meta = extract_result_meta(&mut value);
            (encode_structured(&value), result_meta)
        }
        None => (content_text(&mcp_result.content, "No result"), None),
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
    let state = match convert_tool_result(&request, outcome, spill_dir) {
        Ok((result, result_meta)) => TaskOutcomeState::Completed { result, result_meta },
        Err(error) => TaskOutcomeState::Failed { error },
    };
    TaskOutcome { request, task_id, state }
}

pub(crate) fn call_tool(gateway: Option<&McpGateway>, request: &ToolCallRequest, options: ToolCallOptions) -> ToolCall {
    serde_json::from_str::<Map<String, Value>>(&request.arguments)
        .map_err(McpToolCallError::InvalidArguments)
        .and_then(|arguments| {
            gateway
                .ok_or_else(|| McpError::ToolNotFound(request.name.clone()))
                .and_then(|gateway| gateway.call_tool(&request.name, arguments, options))
                .map_err(McpToolCallError::Unresolved)
        })
        .unwrap_or_else(ToolCall::failed)
}

pub(crate) fn encode_structured(value: &serde_json::Value) -> String {
    noyalib::to_string(value).unwrap_or_else(|_| value.to_string())
}

fn tool_definition(tool: &Tool) -> ToolDefinition {
    let annotations = tool.annotations.as_ref().map(|annotations| ToolAnnotations {
        title: annotations.title.clone(),
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    });
    let definition = ToolDefinition::new(
        tool.name.to_string(),
        tool.description.clone().unwrap_or_default(),
        Value::Object((*tool.input_schema).clone()),
    )
    .with_annotations(annotations);
    match split_namespaced(&tool.name) {
        Some((server, _)) => definition.with_server(server),
        None => definition,
    }
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

fn extract_result_meta(value: &mut serde_json::Value) -> Option<ToolResultMeta> {
    let obj = value.as_object_mut()?;
    let parsed: ToolResultMeta = {
        let meta = obj.get("_meta")?.as_object()?;
        serde_json::from_value(serde_json::Value::Object(meta.clone())).ok()?
    };

    let meta_empty = {
        let meta = obj.get_mut("_meta")?.as_object_mut()?;
        for key in ["display", "file_diff", "plan"] {
            meta.remove(key);
        }
        meta.is_empty()
    };

    if meta_empty {
        obj.remove("_meta");
    }

    Some(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::CallToolResult as McpCallToolResult;
    use serde::Serialize;
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

    #[test]
    fn test_extracts_and_strips_meta() {
        let structured = json!({
            "status": "success", "file_path": "/test/file.rs",
            "_meta": { "display": { "title": "Read file", "value": "file.rs, 50 lines" } }
        });
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.content = vec![ContentBlock::text("plain text fallback")];
        let (result, meta) = convert(mcp).unwrap();

        assert!(!result.result.contains("_meta"));
        assert!(result.result.contains("success"));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Read file");
        assert_eq!(rm.display.value, "file.rs, 50 lines");
        assert!(rm.file_diff.is_none());
    }

    #[test]
    fn test_extracts_meta_with_file_diff() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": {
                "display": { "title": "Edit file", "value": "main.rs" },
                "file_diff": { "path": "/tmp/main.rs", "old_text": "old content", "new_text": "new content" }
            }
        }));
        assert!(!result.result.contains("_meta"));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Edit file");
        let fd = rm.file_diff.expect("file_diff should be present");
        assert_eq!(fd.path, "/tmp/main.rs");
        assert_eq!(fd.old_text.as_deref(), Some("old content"));
        assert_eq!(fd.new_text.as_deref(), Some("new content"));
    }

    #[test]
    fn test_extracts_known_meta_and_preserves_unknown_meta_keys() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": {
                "display": { "title": "Edit file", "value": "main.rs" },
                "file_diff": { "path": "/tmp/main.rs", "old_text": "old", "new_text": "new" },
                "trace_id": "trace-123", "duration_ms": 18
            }
        }));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Edit file");
        assert!(rm.file_diff.is_some());
        for absent in ["display:", "file_diff:"] {
            assert!(!result.result.contains(absent));
        }
        for present in ["trace_id:", "trace-123", "duration_ms:", "18"] {
            assert!(result.result.contains(present));
        }
    }

    #[test]
    fn test_malformed_meta_returns_none() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": { "display": "not a valid ToolDisplayMeta" }
        }));
        assert!(meta.is_none());
        assert!(result.result.contains("not a valid ToolDisplayMeta"));
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
    fn test_extracts_meta_with_plan() {
        let (result, meta) = call_structured(json!({
            "status": "success",
            "_meta": {
                "display": { "title": "Todo", "value": "Research AI agents" },
                "plan": { "entries": [
                    { "content": "Research AI agents", "status": "in_progress" },
                    { "content": "Write tests", "status": "pending" }
                ]}
            }
        }));
        assert!(!result.result.contains("_meta"));
        let rm = meta.expect("meta should be present");
        assert_eq!(rm.display.title, "Todo");
        let plan = rm.plan.expect("plan should be present");
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].content, "Research AI agents");
        assert_eq!(plan.entries[0].status, PlanMetaStatus::InProgress);
        assert_eq!(plan.entries[1].status, PlanMetaStatus::Pending);
    }

    /// Regression: verifies `#[serde(rename = "_meta")]` preserves the key under camelCase,
    /// and that omitting the rename breaks extraction.
    #[test]
    fn test_meta_camel_case_serde_round_trip() {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct GoodResult {
            file_path: String,
            total_lines: usize,
            #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
            _meta: Option<serde_json::Value>,
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct BrokenResult {
            file_path: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            _meta: Option<serde_json::Value>,
        }

        let display_meta = json!({
            "display": { "title": "Read file", "value": "file.rs, 50 lines" }
        });

        let good = serde_json::to_value(&GoodResult {
            file_path: "/test/file.rs".into(),
            total_lines: 50,
            _meta: Some(display_meta.clone()),
        })
        .unwrap();
        assert!(good.get("_meta").is_some(), "expected `_meta` key, got: {good}");
        let (stripped, meta) = call_structured(good);
        let rm = meta.expect("meta should be extracted");
        assert_eq!(rm.display.title, "Read file");
        assert_eq!(rm.display.value, "file.rs, 50 lines");
        assert!(!stripped.result.contains("_meta"));

        let broken =
            serde_json::to_value(&BrokenResult { file_path: "/test/file.rs".into(), _meta: Some(display_meta) })
                .unwrap();
        assert!(broken.get("_meta").is_none(), "should be mangled by camelCase");
        assert!(broken.get("meta").is_some());
        let (_, meta) = call_structured(broken);
        assert!(meta.is_none(), "extraction should fail when _meta is mangled");
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
