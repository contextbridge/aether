//! LSP-powered rename refactoring tool

use lsp_types::{DocumentChangeOperation, DocumentChanges, OneOf, ResourceOp, WorkspaceEdit};
use mcp_utils::display_meta::{ToolDisplayMeta, ToolResultMeta, basename};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::lsp::common::uri_to_path;
use crate::lsp::registry::LspRegistry;

use super::resolve_symbol_position;

/// Input for the `lsp_rename` tool
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspRenameInput {
    /// The file path containing the symbol to rename
    #[serde(alias = "file_path")]
    pub file_path: String,
    /// The symbol name to rename (used for position resolution if line not provided)
    pub symbol: String,
    /// The new name for the symbol
    #[serde(alias = "new_name")]
    pub new_name: String,
    /// Optional 1-indexed line number. When provided, skips automatic symbol resolution.
    #[serde(default)]
    pub line: Option<u32>,
}

/// A single text edit in a file
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FileEdit {
    /// The file path
    pub file_path: String,
    /// The text edits for this file
    pub edits: Vec<TextEdit>,
}

/// A text edit with range information
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    /// 1-indexed start line
    pub start_line: u32,
    /// 1-indexed start column
    pub start_column: u32,
    /// 1-indexed end line
    pub end_line: u32,
    /// 1-indexed end column
    pub end_column: u32,
    /// The new text to insert
    pub new_text: String,
}

/// A raw LSP text edit grouped by file path.
#[derive(Debug, Clone)]
struct LspFileEdit {
    file_path: String,
    edits: Vec<lsp_types::TextEdit>,
}

/// Output from the `lsp_rename` tool
#[derive(Debug, Clone, Default, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LspRenameOutput {
    /// The original symbol name
    pub old_name: String,
    /// The new symbol name
    pub new_name: String,
    /// Total number of edits across all files
    pub total_edits: usize,
    /// Number of files affected
    pub files_affected: usize,
    /// The edits grouped by file
    pub changes: Vec<FileEdit>,
    /// Whether the rename was successful
    pub success: bool,
    /// Error message if rename failed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Display metadata for TUI rendering
    #[serde(rename = "_meta", skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub meta: Option<ToolResultMeta>,
}

/// Execute the rename operation
pub async fn execute_lsp_rename(input: LspRenameInput, registry: &LspRegistry) -> Result<LspRenameOutput, String> {
    let line = resolve_line(&input.file_path, &input.symbol, input.line, registry).await?;

    let resolved = registry.resolve_symbol(&input.file_path, &input.symbol, line).await.map_err(|e| e.to_string())?;

    let workspace_edit = resolved
        .client
        .rename(resolved.uri, resolved.line, resolved.column, input.new_name.clone())
        .await
        .map_err(|e| format!("Rename failed: {e}"))?;

    let Some(edit) = workspace_edit else {
        return Ok(rename_failure(&input, "No changes returned from LSP server".to_string()));
    };

    let raw_changes = match collect_workspace_text_edits(&edit) {
        Ok(changes) => changes,
        Err(error) => return Ok(rename_failure(&input, error)),
    };
    let changes = convert_lsp_file_edits(&raw_changes);

    apply_workspace_text_edits(&raw_changes).await.map_err(|e| format!("Failed to apply rename edits: {e}"))?;
    let total_edits: usize = changes.iter().map(|f| f.edits.len()).sum();
    let files_affected = changes.len();

    Ok(LspRenameOutput {
        old_name: input.symbol.clone(),
        new_name: input.new_name.clone(),
        total_edits,
        files_affected,
        changes,
        success: true,
        meta: Some(rename_display_meta(&input, true, total_edits, files_affected).into()),
        ..Default::default()
    })
}

async fn resolve_line(
    file_path: &str,
    symbol: &str,
    explicit_line: Option<u32>,
    registry: &LspRegistry,
) -> Result<u32, String> {
    match explicit_line {
        Some(line) => Ok(line),
        None => resolve_symbol_position(file_path, symbol, registry).await.map_err(|e| e.to_string()),
    }
}

fn rename_failure(input: &LspRenameInput, error: String) -> LspRenameOutput {
    LspRenameOutput {
        old_name: input.symbol.clone(),
        new_name: input.new_name.clone(),
        success: false,
        error: Some(error),
        meta: Some(rename_display_meta(input, false, 0, 0).into()),
        ..Default::default()
    }
}

/// Convert LSP `WorkspaceEdit` to grouped raw text edits.
fn collect_workspace_text_edits(edit: &WorkspaceEdit) -> Result<Vec<LspFileEdit>, String> {
    let mut result = if let Some(doc_changes) = &edit.document_changes {
        collect_document_changes(doc_changes)?
    } else if let Some(changes) = &edit.changes {
        changes
            .iter()
            .filter(|&(_uri, text_edits)| !text_edits.is_empty())
            .map(|(uri, text_edits)| LspFileEdit { file_path: uri_to_path(uri), edits: text_edits.clone() })
            .collect()
    } else {
        Vec::new()
    };

    result.sort_by(|a, b| a.file_path.cmp(&b.file_path));
    Ok(result)
}

fn collect_document_changes(doc_changes: &DocumentChanges) -> Result<Vec<LspFileEdit>, String> {
    match doc_changes {
        DocumentChanges::Edits(edits) => Ok(edits
            .iter()
            .filter_map(|doc_edit| collect_text_document_edit(&doc_edit.text_document.uri, &doc_edit.edits))
            .collect()),
        DocumentChanges::Operations(ops) => ops
            .iter()
            .map(collect_document_change_operation)
            .filter_map(|result| match result {
                Ok(Some(edit)) => Some(Ok(edit)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect(),
    }
}

fn collect_document_change_operation(op: &DocumentChangeOperation) -> Result<Option<LspFileEdit>, String> {
    match op {
        DocumentChangeOperation::Edit(doc_edit) => {
            Ok(collect_text_document_edit(&doc_edit.text_document.uri, &doc_edit.edits))
        }
        DocumentChangeOperation::Op(ResourceOp::Create(_)) => {
            Err("Rename returned unsupported workspace operation: create".to_string())
        }
        DocumentChangeOperation::Op(ResourceOp::Rename(_)) => {
            Err("Rename returned unsupported workspace operation: rename".to_string())
        }
        DocumentChangeOperation::Op(ResourceOp::Delete(_)) => {
            Err("Rename returned unsupported workspace operation: delete".to_string())
        }
    }
}

fn collect_text_document_edit(
    uri: &lsp_types::Uri,
    edits: &[OneOf<lsp_types::TextEdit, lsp_types::AnnotatedTextEdit>],
) -> Option<LspFileEdit> {
    let edits = edits.iter().map(extract_one_of_text_edit).collect::<Vec<_>>();
    (!edits.is_empty()).then(|| LspFileEdit { file_path: uri_to_path(uri), edits })
}

fn extract_one_of_text_edit(edit: &OneOf<lsp_types::TextEdit, lsp_types::AnnotatedTextEdit>) -> lsp_types::TextEdit {
    match edit {
        OneOf::Left(edit) => edit.clone(),
        OneOf::Right(edit) => edit.text_edit.clone(),
    }
}

fn convert_lsp_file_edits(edits: &[LspFileEdit]) -> Vec<FileEdit> {
    edits
        .iter()
        .map(|file_edit| FileEdit {
            file_path: file_edit.file_path.clone(),
            edits: file_edit.edits.iter().map(convert_text_edit).collect(),
        })
        .collect()
}

async fn apply_workspace_text_edits(changes: &[LspFileEdit]) -> Result<(), String> {
    for file_edit in changes {
        let content = tokio::fs::read_to_string(&file_edit.file_path)
            .await
            .map_err(|e| format!("Failed to read {}: {e}", file_edit.file_path))?;
        let updated = apply_file_text_edits(&content, &file_edit.edits)?;
        tokio::fs::write(&file_edit.file_path, updated)
            .await
            .map_err(|e| format!("Failed to write {}: {e}", file_edit.file_path))?;
    }
    Ok(())
}

fn apply_file_text_edits(content: &str, edits: &[lsp_types::TextEdit]) -> Result<String, String> {
    let mut edits = edits.to_vec();
    edits.sort_by(|a, b| {
        b.range
            .start
            .line
            .cmp(&a.range.start.line)
            .then(b.range.start.character.cmp(&a.range.start.character))
            .then(b.range.end.line.cmp(&a.range.end.line))
            .then(b.range.end.character.cmp(&a.range.end.character))
    });

    let mut result = content.to_string();
    for edit in edits {
        let start = lsp_position_to_byte_offset(&result, edit.range.start)?;
        let end = lsp_position_to_byte_offset(&result, edit.range.end)?;
        if start > end || end > result.len() {
            return Err("Invalid edit range produced by LSP rename".to_string());
        }
        result.replace_range(start..end, &edit.new_text);
    }

    Ok(result)
}

fn lsp_position_to_byte_offset(content: &str, position: lsp_types::Position) -> Result<usize, String> {
    let target_line = usize::try_from(position.line).map_err(|_| format!("Line {} out of range", position.line))?;
    let target_character =
        usize::try_from(position.character).map_err(|_| format!("Character {} out of range", position.character))?;

    let mut line_start = 0usize;
    let mut current_line = 0usize;

    while current_line < target_line {
        let Some(relative_newline) = content[line_start..].find('\n') else {
            return Err(format!("Line {} not found while applying rename", position.line + 1));
        };
        line_start += relative_newline + 1;
        current_line += 1;
    }

    let line_end = content[line_start..].find('\n').map_or(content.len(), |idx| line_start + idx);
    let line = &content[line_start..line_end];

    let mut utf16_units = 0usize;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_units == target_character {
            return Ok(line_start + byte_idx);
        }
        utf16_units += ch.len_utf16();
        if utf16_units > target_character {
            return Err(format!(
                "Character {} splits a UTF-16 code unit sequence on line {}",
                position.character,
                position.line + 1
            ));
        }
    }

    if utf16_units == target_character {
        Ok(line_end)
    } else {
        Err(format!("Character {} out of bounds on line {}", position.character, position.line + 1))
    }
}

fn convert_text_edit(edit: &lsp_types::TextEdit) -> TextEdit {
    TextEdit {
        start_line: edit.range.start.line + 1,
        start_column: edit.range.start.character + 1,
        end_line: edit.range.end.line + 1,
        end_column: edit.range.end.character + 1,
        new_text: edit.new_text.clone(),
    }
}

fn rename_display_meta(
    input: &LspRenameInput,
    success: bool,
    total_edits: usize,
    files_affected: usize,
) -> ToolDisplayMeta {
    let file = basename(&input.file_path);
    if success {
        ToolDisplayMeta::new(
            "LSP rename",
            format!("{} → {} ({} edits in {} files)", input.symbol, input.new_name, total_edits, files_affected),
        )
    } else {
        ToolDisplayMeta::new("LSP rename failed", format!("{} in {}", input.symbol, file))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{
        CreateFile, DocumentChangeOperation, OptionalVersionedTextDocumentIdentifier, Range, ResourceOp,
        TextDocumentEdit, TextEdit as LspTextEdit, Uri,
    };
    use std::collections::HashMap;
    use std::str::FromStr;

    fn make_uri(path: &str) -> Uri {
        Uri::from_str(&format!("file://{path}")).unwrap()
    }

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn test_convert_simple_changes() {
        let mut changes = HashMap::new();
        changes.insert(
            make_uri("/src/main.rs"),
            vec![
                LspTextEdit {
                    range: Range {
                        start: lsp_types::Position { line: 0, character: 5 },
                        end: lsp_types::Position { line: 0, character: 10 },
                    },
                    new_text: "new_name".to_string(),
                },
                LspTextEdit {
                    range: Range {
                        start: lsp_types::Position { line: 5, character: 0 },
                        end: lsp_types::Position { line: 5, character: 8 },
                    },
                    new_text: "new_name".to_string(),
                },
            ],
        );

        let edit = WorkspaceEdit { changes: Some(changes), document_changes: None, change_annotations: None };

        let raw = collect_workspace_text_edits(&edit).unwrap();
        let result = convert_lsp_file_edits(&raw);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_path, "/src/main.rs");
        assert_eq!(result[0].edits.len(), 2);
        assert_eq!(result[0].edits[0].start_line, 1);
        assert_eq!(result[0].edits[0].start_column, 6);
    }

    #[test]
    fn test_convert_document_changes() {
        let doc_edit = TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier { uri: make_uri("/src/lib.rs"), version: Some(1) },
            edits: vec![OneOf::Left(LspTextEdit {
                range: Range {
                    start: lsp_types::Position { line: 10, character: 2 },
                    end: lsp_types::Position { line: 10, character: 6 },
                },
                new_text: "renamed".to_string(),
            })],
        };

        let edit = WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(vec![doc_edit])),
            change_annotations: None,
        };

        let raw = collect_workspace_text_edits(&edit).unwrap();
        let result = convert_lsp_file_edits(&raw);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_path, "/src/lib.rs");
        assert_eq!(result[0].edits[0].start_line, 11);
        assert_eq!(result[0].edits[0].start_column, 3);
        assert_eq!(result[0].edits[0].new_text, "renamed");
    }

    #[test]
    #[allow(clippy::mutable_key_type)]
    fn test_document_changes_take_precedence_over_changes() {
        let mut changes = HashMap::new();
        changes.insert(
            make_uri("/src/changes.rs"),
            vec![LspTextEdit {
                range: Range {
                    start: lsp_types::Position { line: 0, character: 0 },
                    end: lsp_types::Position { line: 0, character: 3 },
                },
                new_text: "old".to_string(),
            }],
        );

        let doc_edit = TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: make_uri("/src/document_changes.rs"),
                version: Some(1),
            },
            edits: vec![OneOf::Left(LspTextEdit {
                range: Range {
                    start: lsp_types::Position { line: 1, character: 0 },
                    end: lsp_types::Position { line: 1, character: 3 },
                },
                new_text: "new".to_string(),
            })],
        };

        let edit = WorkspaceEdit {
            changes: Some(changes),
            document_changes: Some(DocumentChanges::Edits(vec![doc_edit])),
            change_annotations: None,
        };

        let raw = collect_workspace_text_edits(&edit).unwrap();
        let result = convert_lsp_file_edits(&raw);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_path, "/src/document_changes.rs");
    }

    #[test]
    fn test_rejects_unsupported_resource_operations() {
        let edit = WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Operations(vec![DocumentChangeOperation::Op(ResourceOp::Create(
                CreateFile { uri: make_uri("/src/new.rs"), options: None, annotation_id: None },
            ))])),
            change_annotations: None,
        };

        let err = collect_workspace_text_edits(&edit).unwrap_err();
        assert!(err.contains("unsupported workspace operation"));
        assert!(err.contains("create"));
    }

    #[test]
    fn test_apply_file_text_edits_replaces_multiple_ranges_from_end() {
        let content = "fn greet() {\n    greet();\n    greet();\n}\n";
        let edits = vec![
            LspTextEdit {
                range: Range {
                    start: lsp_types::Position { line: 0, character: 3 },
                    end: lsp_types::Position { line: 0, character: 8 },
                },
                new_text: "hello".to_string(),
            },
            LspTextEdit {
                range: Range {
                    start: lsp_types::Position { line: 1, character: 4 },
                    end: lsp_types::Position { line: 1, character: 9 },
                },
                new_text: "hello".to_string(),
            },
            LspTextEdit {
                range: Range {
                    start: lsp_types::Position { line: 2, character: 4 },
                    end: lsp_types::Position { line: 2, character: 9 },
                },
                new_text: "hello".to_string(),
            },
        ];

        let updated = apply_file_text_edits(content, &edits).unwrap();
        assert_eq!(updated, "fn hello() {\n    hello();\n    hello();\n}\n");
    }

    #[test]
    fn test_lsp_position_to_byte_offset_handles_utf16_columns() {
        let content = "a😀z\n";

        assert_eq!(lsp_position_to_byte_offset(content, lsp_types::Position { line: 0, character: 0 }).unwrap(), 0);
        assert_eq!(lsp_position_to_byte_offset(content, lsp_types::Position { line: 0, character: 1 }).unwrap(), 1);
        assert_eq!(lsp_position_to_byte_offset(content, lsp_types::Position { line: 0, character: 3 }).unwrap(), 5);
        assert_eq!(lsp_position_to_byte_offset(content, lsp_types::Position { line: 0, character: 4 }).unwrap(), 6);
    }
}
