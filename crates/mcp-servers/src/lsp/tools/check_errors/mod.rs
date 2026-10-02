//! LSP diagnostics tool for querying compiler errors and warnings

// `Uri` only uses interior mutability to cache parsed components; its identity is stable.
#![allow(clippy::mutable_key_type)]

use crate::lsp::common::truncate_results;
use crate::lsp::diagnostics::{DiagnosticCounts, FormattedDiagnostic, Severity, count_by_severity};
use crate::lsp::error::LspError;
use crate::lsp::registry::LspRegistry;
use crate::workspace_paths::relative_path;
use lsp_types::Diagnostic;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};

/// Input payload for the `lsp_check_errors` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LspDiagnosticsRequest {
    /// Path to an existing file, absolute or relative to the workspace root. When omitted, checks the workspace.
    #[serde(default, alias = "file_path")]
    pub file_path: Option<String>,
}

/// Output from the `lsp_check_errors` tool
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspDiagnosticsOutput {
    pub scope: Scope,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_path: Option<String>,
    pub diagnostics: BTreeMap<String, Vec<String>>,
    pub summary: DiagnosticCounts,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncated: Option<bool>,

    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

pub const MAX_LISTED_DIAGNOSTICS: usize = 200;

/// Scope label for output serialization
#[derive(Debug, Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Workspace,
    File,
}

impl LspDiagnosticsOutput {
    pub fn new(
        request: &LspDiagnosticsRequest,
        root_path: &Path,
        diagnostics_cache: &HashMap<lsp_types::Uri, Vec<Diagnostic>>,
    ) -> Self {
        let mut diagnostics: Vec<FormattedDiagnostic> = diagnostics_cache
            .iter()
            .flat_map(|(uri, diagnostics)| {
                diagnostics.iter().map(move |diagnostic| FormattedDiagnostic::from_diagnostic(uri, diagnostic))
            })
            .collect();

        let summary = count_by_severity(&diagnostics);
        diagnostics.retain(|diagnostic| diagnostic.severity <= Severity::Warning);
        diagnostics
            .sort_by(|a, b| (a.severity, &a.file, a.line, a.column).cmp(&(b.severity, &b.file, b.line, b.column)));

        let truncation = truncate_results(&mut diagnostics, MAX_LISTED_DIAGNOSTICS);
        let mut by_file: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for diagnostic in &diagnostics {
            by_file.entry(relative_path(root_path, &diagnostic.file)).or_default().push(diagnostic.to_string());
        }

        let file_path = request.file_path.clone();
        let is_workspace = file_path.is_none();

        Self {
            scope: if is_workspace { Scope::Workspace } else { Scope::File },
            workspace_root: if is_workspace { Some(root_path.to_string_lossy().to_string()) } else { None },
            file_path,
            diagnostics: by_file,
            summary,
            truncated: truncation.truncated,
            meta: None,
        }
    }
}

/// Execute the `lsp_check_errors` operation
pub async fn execute_lsp_diagnostics(
    mut request: LspDiagnosticsRequest,
    registry: &LspRegistry,
) -> Result<LspDiagnosticsOutput, LspError> {
    request.file_path = request.file_path.map(|path| registry.resolve_file(&path)).transpose()?;
    if let Some(file_path) = request.file_path.as_deref().filter(|path| !Path::new(path).is_file()) {
        return Err(LspError::InvalidPath(format!("filePath must point to an existing file, got: {file_path}")));
    }

    let diagnostics_cache = registry.collect_diagnostics(request.file_path.as_deref()).await?;
    let mut output = LspDiagnosticsOutput::new(&request, registry.root_path(), &diagnostics_cache);

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
        LspDiagnosticsOutput::new(&workspace_request(), Path::new("/project"), cache)
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

        assert_eq!(
            result.diagnostics["src/main.rs"],
            ["11:1 error[E0308]: type mismatch", "21:1 warning[E0308]: unused variable"]
        );
        assert_eq!(result.diagnostics["src/lib.rs"], ["6:1 error[E0308]: missing field"]);
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
        let result = LspDiagnosticsOutput::new(&input, Path::new("/project"), &cache);

        assert_eq!(result.diagnostics["src/main.rs"], ["11:1 error[E0308]: type mismatch"]);
        assert_eq!(result.summary.total, 1);
    }

    #[test]
    fn multiline_messages_are_listed_on_one_line() {
        let cache = HashMap::from([(
            uri("/project/src/main.rs"),
            vec![diag(DiagnosticSeverity::ERROR, "mismatched types\nexpected `u32`, found `&str`", 1)],
        )]);

        let result = workspace_output(&cache);

        assert_eq!(
            result.diagnostics["src/main.rs"],
            ["2:1 error[E0308]: mismatched types expected `u32`, found `&str`"]
        );
    }

    #[test]
    fn test_empty_diagnostics() {
        let result = workspace_output(&HashMap::new());
        assert!(result.diagnostics.is_empty());
        assert_eq!(result.summary.total, 0);
    }

    #[test]
    fn test_diagnostics_sorted() {
        let mut cache = HashMap::new();
        cache.insert(uri("/project/src/b.rs"), vec![diag(DiagnosticSeverity::ERROR, "error in b", 5)]);
        cache.insert(uri("/project/src/a.rs"), vec![diag(DiagnosticSeverity::ERROR, "error in a", 10)]);

        let result = workspace_output(&cache);
        let files: Vec<&str> = result.diagnostics.keys().map(String::as_str).collect();
        assert_eq!(files, ["src/a.rs", "src/b.rs"]);
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

    #[tokio::test]
    async fn rejects_file_paths_that_are_not_existing_files() {
        let temp_dir = TempDir::new().unwrap();
        std::fs::create_dir(temp_dir.path().join("src")).unwrap();
        let registry = LspRegistry::new(temp_dir.path().to_path_buf());

        let cases = [
            ("", "file path is required"),
            ("src", "filePath must point to an existing file"),
            ("src/missing.rs", "filePath must point to an existing file"),
        ];

        for (path, expected_msg) in cases {
            let err = execute_lsp_diagnostics(file_request(path), &registry).await.unwrap_err().to_string();
            assert!(err.contains(expected_msg), "path={path:?}: expected {expected_msg:?}, got {err:?}");
        }
    }

    #[test]
    fn test_output_workspace_metadata() {
        let output = LspDiagnosticsOutput::new(&workspace_request(), Path::new("/home/user/project"), &HashMap::new());

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains(r#""scope":"workspace""#));
        assert!(json.contains(r#""workspaceRoot":"/home/user/project""#));
        assert!(!json.contains("filePath"));
    }

    #[test]
    fn test_output_file_metadata() {
        let input = file_request("/home/user/project/src/main.rs");
        let output = LspDiagnosticsOutput::new(&input, Path::new("/home/user/project"), &HashMap::new());

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
        assert_eq!(
            result.diagnostics["src/main.rs"],
            ["2:1 error[E0308]: error1", "3:1 error[E0308]: error2", "4:1 warning[E0308]: warn1"]
        );
    }

    #[test]
    fn test_listed_diagnostics_are_capped_with_errors_first() {
        let line_count = u32::try_from(MAX_LISTED_DIAGNOSTICS).unwrap();
        let mut diagnostics: Vec<Diagnostic> =
            (0..line_count).map(|line| diag(DiagnosticSeverity::WARNING, "warn", line)).collect();
        diagnostics.push(diag(DiagnosticSeverity::ERROR, "late error", line_count + 10));
        let cache = HashMap::from([(uri("/project/src/main.rs"), diagnostics)]);

        let result = workspace_output(&cache);

        let listed = &result.diagnostics["src/main.rs"];
        assert_eq!(listed.len(), MAX_LISTED_DIAGNOSTICS);
        assert_eq!(listed[0], format!("{}:1 error[E0308]: late error", line_count + 11));
        assert_eq!(result.truncated, Some(true));
        assert_eq!(result.summary.total, MAX_LISTED_DIAGNOSTICS + 1);
    }
}
