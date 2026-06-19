use std::path::{Path, PathBuf};

use mcp_utils::{client::SERVERNAME_DELIMITER, display_meta::ToolResultMeta};
use rmcp::model::CallToolRequestParams;
use serde_json;

use llm::{ToolCallError, ToolCallRequest, ToolCallResult};

/// Maximum bytes for a tool result before spilling to disk.
/// ~50K tokens at ~4 bytes/token.
const TOOL_RESULT_MAX_BYTES: usize = 200_000;

/// Size of the head preview included inline when a result spills to disk.
const SPILLOVER_PREVIEW_BYTES: usize = 10_000;

/// Convert a `ToolCallRequest` to `rmcp::CallToolRequestParams`
pub fn tool_call_request_to_mcp(request: &ToolCallRequest) -> Result<CallToolRequestParams, String> {
    let tool_name = request
        .name
        .split_once(SERVERNAME_DELIMITER)
        .map_or_else(|| request.name.clone(), |(_, tool_name)| tool_name.to_string());

    // Parse arguments from JSON string
    let arguments = serde_json::from_str::<serde_json::Value>(&request.arguments)
        .map_err(|e| format!("Invalid tool arguments: {e}"))?
        .as_object()
        .cloned();

    let mut params = CallToolRequestParams::new(tool_name);
    if let Some(args) = arguments {
        params = params.with_arguments(args);
    }
    Ok(params)
}

/// Convert an rmcp `CallToolResult` and request to `ToolCallResult` or `ToolCallError`,
/// extracting any `_meta` metadata from structured content.
pub fn mcp_result_to_tool_call_result(
    request: &ToolCallRequest,
    mcp_result: rmcp::model::CallToolResult,
) -> Result<(ToolCallResult, Option<ToolResultMeta>), ToolCallError> {
    if mcp_result.is_error.unwrap_or(false) {
        let error_msg =
            mcp_result.content.first().map_or_else(|| "Unknown error".to_string(), |content| format!("{content:?}"));
        Err(ToolCallError {
            id: request.id.clone(),
            name: request.name.clone(),
            arguments: Some(request.arguments.clone()),
            error: format!("Tool execution error: {error_msg}"),
        })
    } else {
        let (result_value, result_meta) = extract_result_and_meta(mcp_result.structured_content, &mcp_result.content);
        // YAML is ~18% more token-efficient than JSON for LLM consumption
        let yaml = serde_yml::to_string(&result_value).unwrap_or_else(|_| result_value.to_string());
        let result_str = maybe_spillover(&request.id, yaml, TOOL_RESULT_MAX_BYTES, &spillover_dir());
        Ok((
            ToolCallResult {
                id: request.id.clone(),
                name: request.name.clone(),
                arguments: request.arguments.clone(),
                result: result_str,
            },
            result_meta,
        ))
    }
}

fn spillover_dir() -> PathBuf {
    std::env::temp_dir().join("aether-tool-output")
}

/// If `result` exceeds `max_bytes`, write the full output to disk and return
/// a head preview with a pointer to the file. Otherwise return unchanged.
fn maybe_spillover(tool_id: &str, result: String, max_bytes: usize, dir: &Path) -> String {
    if result.len() <= max_bytes {
        return result;
    }

    if let Err(e) = std::fs::create_dir_all(dir) {
        tracing::warn!("Failed to create tool-output dir: {e}");
        return result;
    }

    let file_path = dir.join(format!("{tool_id}.txt"));

    if let Err(e) = std::fs::write(&file_path, &result) {
        tracing::warn!("Failed to write spillover file: {e}");
        return result;
    }

    let preview_end = result.floor_char_boundary(SPILLOVER_PREVIEW_BYTES);
    let preview = &result[..preview_end];
    let total_bytes = result.len();

    format!(
        "<preview>\n{preview}\n</preview>\n\n[Tool result too large ({total_bytes} bytes). Full output saved to {path}. Use grep, read, or tail to explore the full result.]",
        path = file_path.display()
    )
}

fn extract_result_and_meta(
    structured_content: Option<serde_json::Value>,
    content: &[rmcp::model::Content],
) -> (serde_json::Value, Option<ToolResultMeta>) {
    if let Some(mut val) = structured_content {
        let result_meta = extract_result_meta(&mut val);
        (val, result_meta)
    } else {
        let fallback = content.first().map_or_else(
            || serde_json::Value::String("No result".to_string()),
            |c| serde_json::to_value(c).unwrap_or(serde_json::Value::String("Serialization error".to_string())),
        );
        (fallback, None)
    }
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
    use mcp_utils::display_meta::PlanMetaStatus;
    use rmcp::model::{CallToolResult as McpCallToolResult, Content};
    use serde::Serialize;
    use serde_json::json;

    fn req() -> ToolCallRequest {
        ToolCallRequest { id: "call_123".into(), name: "test_tool".into(), arguments: "{}".into() }
    }

    fn call_structured(structured: serde_json::Value) -> (ToolCallResult, Option<ToolResultMeta>) {
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.content = vec![];
        mcp_result_to_tool_call_result(&req(), mcp).unwrap()
    }

    fn extract_preview(result: &str) -> &str {
        let start = result.find("<preview>\n").unwrap() + "<preview>\n".len();
        let end = result.find("\n</preview>").unwrap();
        &result[start..end]
    }

    #[test]
    fn test_extracts_and_strips_meta() {
        let structured = json!({
            "status": "success", "file_path": "/test/file.rs",
            "_meta": { "display": { "title": "Read file", "value": "file.rs, 50 lines" } }
        });
        let mut mcp = McpCallToolResult::structured(structured);
        mcp.content = vec![Content::text("plain text fallback")];
        let (result, meta) = mcp_result_to_tool_call_result(&req(), mcp).unwrap();

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
        assert_eq!(fd.new_text, "new content");
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
    fn test_tool_call_result_falls_back_to_content() {
        let mcp = McpCallToolResult::success(vec![Content::text("plain text result")]);
        let (result, meta) = mcp_result_to_tool_call_result(&req(), mcp).unwrap();
        assert!(result.result.contains("plain text result"));
        assert!(meta.is_none());
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
        let (stripped, meta) = extract_result_and_meta(Some(good), &[]);
        let rm = meta.expect("meta should be extracted");
        assert_eq!(rm.display.title, "Read file");
        assert_eq!(rm.display.value, "file.rs, 50 lines");
        assert!(stripped.get("_meta").is_none());

        let broken =
            serde_json::to_value(&BrokenResult { file_path: "/test/file.rs".into(), _meta: Some(display_meta) })
                .unwrap();
        assert!(broken.get("_meta").is_none(), "should be mangled by camelCase");
        assert!(broken.get("meta").is_some());
        let (_, meta) = extract_result_and_meta(Some(broken), &[]);
        assert!(meta.is_none(), "extraction should fail when _meta is mangled");
    }

    #[test]
    fn test_tool_call_result_handles_error() {
        let mcp = McpCallToolResult::error(vec![Content::text("Error: file not found")]);
        let err = mcp_result_to_tool_call_result(&req(), mcp).unwrap_err();
        assert!(err.error.contains("file not found"));
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
    fn test_serde_yml_produces_yaml_not_json() {
        let yaml = serde_yml::to_string(&json!({"key": "value"})).unwrap();
        assert!(yaml.contains("key:") && yaml.contains("value") && !yaml.starts_with('{'));
    }

    #[test]
    fn test_spillover_small_input_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let input = "hello world".to_string();
        assert_eq!(maybe_spillover("id", input.clone(), 1000, dir.path()), input);
    }

    #[test]
    fn test_spillover_large_input_writes_file() {
        let dir = tempfile::tempdir().unwrap();
        let large = "x".repeat(5000);
        let result = maybe_spillover("test_large", large.clone(), 1000, dir.path());
        for expected in ["<preview>", "</preview>", "Tool result too large", "5000 bytes", "test_large.txt"] {
            assert!(result.contains(expected), "missing '{expected}' in: {result}");
        }
        let on_disk = std::fs::read_to_string(dir.path().join("test_large.txt")).unwrap();
        assert_eq!(on_disk, large);
    }

    #[test]
    fn test_spillover_preview_content() {
        let dir = tempfile::tempdir().unwrap();
        let large = format!("HEAD_{}{}", "z".repeat(SPILLOVER_PREVIEW_BYTES + 5000), "TAIL");
        let result = maybe_spillover("id", large, 1000, dir.path());
        assert!(result.contains("HEAD_"));
        assert!(!result.contains("TAIL"));
    }

    #[test]
    fn test_spillover_preserves_utf8_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let large = format!("{}{}", "\u{1F600}".repeat(300), "a".repeat(5000));
        let result = maybe_spillover("id", large, 100, dir.path());
        assert!(extract_preview(&result).chars().count() > 0);
    }

    #[test]
    fn test_mcp_result_spills_large_output() {
        let request =
            ToolCallRequest { id: "spill_integration".into(), name: "big_tool".into(), arguments: "{}".into() };
        let mut mcp = McpCallToolResult::structured(json!({"data": "x".repeat(TOOL_RESULT_MAX_BYTES + 1000)}));
        mcp.content = vec![];
        let (result, _) = mcp_result_to_tool_call_result(&request, mcp).unwrap();
        for expected in ["<preview>", "Tool result too large", "spill_integration.txt"] {
            assert!(result.result.contains(expected));
        }
    }
}
