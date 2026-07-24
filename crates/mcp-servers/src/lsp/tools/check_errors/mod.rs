//! LSP diagnostics tool for querying compiler errors and warnings

// `Uri` only uses interior mutability to cache parsed components; its identity is stable.
#![allow(clippy::mutable_key_type)]

use crate::lsp::diagnostics::{DiagnosticCounts, FormattedDiagnostic, count_by_severity};
use crate::lsp::error::LspError;
use crate::lsp::registry::LspRegistry;
use lsp_types::Diagnostic;
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Input payload for the `lsp_check_errors` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LspDiagnosticsRequest {
    /// Absolute path to a file. When omitted, checks the workspace.
    #[serde(default, alias = "file_path")]
    pub file_path: Option<String>,
}

impl LspDiagnosticsRequest {
    fn validate(&self) -> Result<(), LspError> {
        if let Some(file_path) = &self.file_path {
            if file_path.trim().is_empty() {
                return Err(LspError::InvalidPath("filePath cannot be empty".to_string()));
            }
            let path = Path::new(file_path);
            if !path.is_absolute() {
                return Err(LspError::InvalidPath(format!("filePath must be an absolute path, got: {file_path}")));
            }
            if !path.is_file() {
                return Err(LspError::InvalidPath(format!(
                    "filePath must point to an existing file, got: {file_path}"
                )));
            }
        }
        Ok(())
    }
}

/// Output from the `lsp_check_errors` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsOutput {
    /// The scope that was queried
    pub scope: Scope,
    /// The workspace root (present when scope is "workspace")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,
    /// The file path that was queried (present when scope is "file")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    /// List of diagnostics
    pub diagnostics: Vec<FormattedDiagnostic>,
    /// Summary counts
    pub summary: DiagnosticCounts,
    /// Display metadata for human-friendly rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Scope label for output serialization
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Workspace,
    File,
}

/// Execute the `lsp_check_errors` operation
pub async fn execute_lsp_diagnostics(
    request: LspDiagnosticsRequest,
    registry: &LspRegistry,
) -> Result<LspDiagnosticsOutput, LspError> {
    request.validate()?;

    let diagnostics_cache = registry.collect_diagnostics(request.file_path.as_deref()).await?;
    let mut output = build_output(&request, registry.root_path(), &diagnostics_cache);

    let detail = if output.summary.errors == 0 && output.summary.warnings == 0 {
        "no issues".to_string()
    } else {
        format!("{} errors, {} warnings", output.summary.errors, output.summary.warnings)
    };
    let value = match &output.file_path {
        Some(fp) => format!("{}, {detail}", basename(fp)),
        None => detail,
    };
    #[allow(clippy::used_underscore_binding)]
    {
        output.meta = Some(ToolDisplayMeta::new("LSP errors", value).into());
    }

    Ok(output)
}

fn build_output(
    input: &LspDiagnosticsRequest,
    root_path: &Path,
    diagnostics_cache: &HashMap<lsp_types::Uri, Vec<Diagnostic>>,
) -> LspDiagnosticsOutput {
    let mut diagnostics: Vec<FormattedDiagnostic> = diagnostics_cache
        .iter()
        .flat_map(|(uri, diagnostics)| {
            diagnostics.iter().map(move |diagnostic| FormattedDiagnostic::from_diagnostic(uri, diagnostic))
        })
        .collect();

    diagnostics.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)).then(a.column.cmp(&b.column)));

    let summary = count_by_severity(&diagnostics);
    let file_path = input.file_path.clone();
    let is_workspace = file_path.is_none();

    LspDiagnosticsOutput {
        scope: if is_workspace { Scope::Workspace } else { Scope::File },
        workspace_root: if is_workspace { Some(root_path.to_string_lossy().to_string()) } else { None },
        file_path,
        diagnostics,
        summary,
        meta: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{DiagnosticSeverity, Position, Range};
    use tempfile::TempDir;

    fn uri(path: &str) -> lsp_types::Uri {
        format!("file://{path}").parse().unwrap()
    }

    fn diag(severity: DiagnosticSeverity, message: &str, line: u32) -> Diagnostic {
        Diagnostic {
            range: Range { start: Position { line, character: 0 }, end: Position { line, character: 10 } },
            severity: Some(severity),
            code: Some(lsp_types::NumberOrString::String("E0308".to_string())),
            code_description: None,
            source: Some("rustc".to_string()),
            message: message.to_string(),
            related_information: None,
            tags: None,
            data: None,
        }
    }

    fn workspace_request() -> LspDiagnosticsRequest {
        LspDiagnosticsRequest { file_path: None }
    }

    fn file_request(path: &str) -> LspDiagnosticsRequest {
        LspDiagnosticsRequest { file_path: Some(path.to_string()) }
    }

    fn workspace_output(cache: &HashMap<lsp_types::Uri, Vec<Diagnostic>>) -> LspDiagnosticsOutput {
        build_output(&workspace_request(), Path::new("/project"), cache)
    }

    fn parse_request(json: &str) -> Result<LspDiagnosticsRequest, serde_json::Error> {
        serde_json::from_str(json)
    }

    fn assert_parses_file_scope(json: &str, expected_path: &str) {
        let request: LspDiagnosticsRequest = parse_request(json).unwrap();
        assert_eq!(request.file_path.as_deref(), Some(expected_path));
    }

    #[test]
    fn test_get_all_diagnostics() {
        let mut cache = HashMap::new();
        cache.insert(
            uri("/project/src/main.rs"),
            vec![
                diag(DiagnosticSeverity::ERROR, "type mismatch", 10),
                diag(DiagnosticSeverity::WARNING, "unused variable", 20),
            ],
        );
        cache.insert(uri("/project/src/lib.rs"), vec![diag(DiagnosticSeverity::ERROR, "missing field", 5)]);

        let result = workspace_output(&cache);

        assert_eq!(result.diagnostics.len(), 3);
        assert_eq!(result.summary.errors, 2);
        assert_eq!(result.summary.warnings, 1);
        assert_eq!(result.summary.infos, 0);
        assert_eq!(result.summary.hints, 0);
        assert_eq!(result.summary.total, 3);
    }

    #[test]
    fn test_get_diagnostics_for_file() {
        let mut cache = HashMap::new();
        cache.insert(uri("/project/src/main.rs"), vec![diag(DiagnosticSeverity::ERROR, "type mismatch", 10)]);

        let input = file_request("/project/src/main.rs");
        let result = build_output(&input, Path::new("/project"), &cache);

        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0].file.contains("main.rs"));
        assert_eq!(result.diagnostics[0].message, "type mismatch");
        assert_eq!(result.summary.total, 1);
    }

    #[test]
    fn test_empty_diagnostics() {
        let result = workspace_output(&HashMap::new());
        assert_eq!(result.diagnostics.len(), 0);
        assert_eq!(result.summary.total, 0);
    }

    #[test]
    fn test_diagnostics_sorted() {
        let mut cache = HashMap::new();
        cache.insert(uri("/project/src/b.rs"), vec![diag(DiagnosticSeverity::ERROR, "error in b", 5)]);
        cache.insert(uri("/project/src/a.rs"), vec![diag(DiagnosticSeverity::ERROR, "error in a", 10)]);

        let result = workspace_output(&cache);
        assert!(result.diagnostics[0].file.contains("a.rs"));
        assert!(result.diagnostics[1].file.contains("b.rs"));
    }

    #[test]
    fn test_deserialize_workspace_scope() {
        let request: LspDiagnosticsRequest = parse_request(r"{}").unwrap();
        assert!(request.file_path.is_none());
    }

    #[test]
    fn test_deserialize_file_scope() {
        let cases = [r#"{"filePath":"/some/path.rs"}"#, r#"{"file_path":"/some/path.rs"}"#];
        for json in cases {
            assert_parses_file_scope(json, "/some/path.rs");
        }
    }

    #[test]
    fn test_reject_invalid_json_payloads() {
        let invalid_jsons = [r#"{"scope":"workspace"}"#, r#"{"scope":"file"}"#, r#"{"unknown":true}"#];
        for json in invalid_jsons {
            assert!(parse_request(json).is_err(), "should reject: {json}");
        }
    }

    #[test]
    fn test_validate_rejects_bad_file_paths() {
        let temp_dir = TempDir::new().unwrap();
        let dir_path = temp_dir.path().to_string_lossy().to_string();
        let missing_path = temp_dir.path().join("missing.rs");
        let missing = missing_path.to_string_lossy().to_string();

        let cases: Vec<(&str, &str)> = vec![
            ("", "filePath cannot be empty"),
            ("src/main.rs", "filePath must be an absolute path"),
            (&dir_path, "filePath must point to an existing file"),
            (&missing, "filePath must point to an existing file"),
        ];

        for (path, expected_msg) in cases {
            let input = file_request(path);
            let err = input.validate().unwrap_err().to_string();
            assert!(err.contains(expected_msg), "path={path:?}: expected {expected_msg:?}, got {err:?}");
        }
    }

    #[test]
    fn test_output_workspace_metadata() {
        let output = build_output(&workspace_request(), Path::new("/home/user/project"), &HashMap::new());

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains(r#""scope":"workspace""#));
        assert!(json.contains(r#""workspaceRoot":"/home/user/project""#));
        assert!(!json.contains("filePath"));
    }

    #[test]
    fn test_output_file_metadata() {
        let input = file_request("/home/user/project/src/main.rs");
        let output = build_output(&input, Path::new("/home/user/project"), &HashMap::new());

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains(r#""scope":"file""#));
        assert!(json.contains(r#""filePath":"/home/user/project/src/main.rs""#));
        assert!(!json.contains("workspaceRoot"));
        assert!(output.workspace_root.is_none());
    }

    #[test]
    fn test_output_summary_totals() {
        let mut cache = HashMap::new();
        cache.insert(
            uri("/project/src/main.rs"),
            vec![
                diag(DiagnosticSeverity::ERROR, "error1", 1),
                diag(DiagnosticSeverity::ERROR, "error2", 2),
                diag(DiagnosticSeverity::WARNING, "warn1", 3),
                diag(DiagnosticSeverity::INFORMATION, "info1", 4),
                diag(DiagnosticSeverity::HINT, "hint1", 5),
            ],
        );

        let result = workspace_output(&cache);
        assert_eq!(result.summary.errors, 2);
        assert_eq!(result.summary.warnings, 1);
        assert_eq!(result.summary.infos, 1);
        assert_eq!(result.summary.hints, 1);
        assert_eq!(result.summary.total, 5);
    }
}
