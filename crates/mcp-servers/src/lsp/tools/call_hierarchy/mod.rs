//! LSP call hierarchy tool
//!
//! This module provides call hierarchy operations:
//! - incoming: Find functions/methods that call the given item
//! - outgoing: Find functions/methods that the given item calls

use std::collections::HashMap;
use std::path::Path;

use lsp_types::CallHierarchyItem;
use schemars::JsonSchema;
use serde::Serialize;

use crate::lsp::common::{LocationResult, display_path, enrich_locations, is_project_local, uri_to_path};
use aether_lspd::symbol_kind_to_string;

/// A serializable representation of a `CallHierarchyItem`
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallHierarchyItemResult {
    /// The name of the symbol
    pub name: String,
    /// The kind of the symbol (function, method, etc.)
    pub kind: String,
    /// Additional detail (e.g., signature)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// The file path containing this symbol
    pub file_path: String,
    /// Compact path for display. Dependency store encodings are removed.
    pub display_path: String,
    /// The range of the entire symbol
    pub range: LocationResult,
    /// The range of the symbol name
    pub selection_range: LocationResult,
}

impl CallHierarchyItemResult {
    fn new(item: CallHierarchyItem, project_root: &Path) -> Self {
        let file_path = uri_to_path(&item.uri);
        let range = LocationResult::from_range(file_path.clone(), &item.range);
        let selection_range = LocationResult::from_range(file_path.clone(), &item.selection_range);
        Self {
            name: item.name,
            kind: symbol_kind_to_string(item.kind).to_string(),
            detail: item.detail,
            display_path: display_path(&file_path, project_root),
            file_path,
            range,
            selection_range,
        }
    }
}

/// A call site result
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CallSiteResult {
    /// The item making or receiving the call
    pub item: CallHierarchyItemResult,
    /// Whether the called/calling item is inside the current project.
    pub project_local: bool,
    /// The locations where calls occur
    pub call_sites: Vec<LocationResult>,
}

/// Convert LSP incoming calls to serializable `CallSiteResult`s.
pub fn convert_incoming_calls(
    incoming: Vec<lsp_types::CallHierarchyIncomingCall>,
    project_root: &Path,
) -> Vec<CallSiteResult> {
    incoming.into_iter().map(|call| convert_call(call.from, &call.from_ranges, None, project_root)).collect()
}

/// Convert LSP outgoing calls to serializable `CallSiteResult`s.
///
/// Outgoing `from_ranges` belong to the source item, not the callee.
pub fn convert_outgoing_calls(
    source_file_path: &str,
    project_root: &Path,
    outgoing: Vec<lsp_types::CallHierarchyOutgoingCall>,
) -> Vec<CallSiteResult> {
    outgoing
        .into_iter()
        .map(|call| convert_call(call.to, &call.from_ranges, Some(source_file_path), project_root))
        .collect()
}

/// Merge duplicate hierarchy items and ranges, then sort local results first.
pub fn normalize_calls(calls: Vec<CallSiteResult>) -> Vec<CallSiteResult> {
    let mut merged: HashMap<CallItemKey, CallSiteResult> = HashMap::new();
    for mut call in calls {
        let key = CallItemKey::from(&call.item);
        if let Some(existing) = merged.get_mut(&key) {
            existing.call_sites.append(&mut call.call_sites);
        } else {
            merged.insert(key, call);
        }
    }

    let mut calls: Vec<_> = merged.into_values().collect();
    for call in &mut calls {
        deduplicate_ranges(&mut call.call_sites);
    }
    calls.sort_by(|left, right| {
        right
            .project_local
            .cmp(&left.project_local)
            .then_with(|| left.item.display_path.cmp(&right.item.display_path))
            .then_with(|| left.item.name.cmp(&right.item.name))
    });
    calls
}

/// Add source context to call sites when requested.
pub async fn enrich_call_sites(calls: &mut [CallSiteResult], context_lines: Option<u32>) {
    let Some(context_lines) = context_lines.filter(|lines| *lines > 0) else {
        return;
    };

    for call in calls {
        enrich_locations(&mut call.call_sites, context_lines).await;
    }
}

#[derive(Debug, Eq, Hash, PartialEq)]
struct CallItemKey {
    name: String,
    file_path: String,
    start_line: u32,
    start_column: u32,
}

impl From<&CallHierarchyItemResult> for CallItemKey {
    fn from(item: &CallHierarchyItemResult) -> Self {
        Self {
            name: item.name.clone(),
            file_path: item.file_path.clone(),
            start_line: item.selection_range.start_line,
            start_column: item.selection_range.start_column,
        }
    }
}

fn convert_call(
    item: CallHierarchyItem,
    from_ranges: &[lsp_types::Range],
    source_file_path: Option<&str>,
    project_root: &Path,
) -> CallSiteResult {
    let call_site_path = source_file_path.map_or_else(|| uri_to_path(&item.uri), ToOwned::to_owned);
    let item = CallHierarchyItemResult::new(item, project_root);
    let call_sites =
        from_ranges.iter().map(|range| LocationResult::from_range(call_site_path.clone(), range)).collect();
    CallSiteResult { project_local: is_project_local(&item.file_path, project_root), item, call_sites }
}

fn deduplicate_ranges(ranges: &mut Vec<LocationResult>) {
    ranges.sort_by_key(|range| (range.start_line, range.start_column));
    ranges.dedup_by_key(|range| (range.start_line, range.start_column));
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use lsp_types::{CallHierarchyIncomingCall, CallHierarchyItem, CallHierarchyOutgoingCall};

    fn make_item(name: &str, line: u32) -> CallHierarchyItem {
        CallHierarchyItem {
            name: name.to_string(),
            kind: lsp_types::SymbolKind::FUNCTION,
            tags: None,
            detail: None,
            uri: lsp_types::Uri::from_str("file:///src/lib.rs").unwrap(),
            range: lsp_types::Range {
                start: lsp_types::Position { line, character: 0 },
                end: lsp_types::Position { line: line + 5, character: 1 },
            },
            selection_range: lsp_types::Range {
                start: lsp_types::Position { line, character: 3 },
                end: lsp_types::Position {
                    line,
                    character: 3 + u32::try_from(name.len()).expect("symbol name too long"),
                },
            },
            data: None,
        }
    }

    fn make_range(line: u32, col: u32) -> lsp_types::Range {
        lsp_types::Range {
            start: lsp_types::Position { line, character: col },
            end: lsp_types::Position { line, character: col + 5 },
        }
    }

    #[test]
    fn test_convert_incoming_calls() {
        let incoming = vec![CallHierarchyIncomingCall {
            from: make_item("caller_fn", 10),
            from_ranges: vec![make_range(12, 4), make_range(14, 8)],
        }];

        let result = convert_incoming_calls(incoming, Path::new("/src"));
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item.name, "caller_fn");
        assert!(result[0].project_local);
        assert_eq!(result[0].item.display_path, "lib.rs");
        assert_eq!(result[0].call_sites.len(), 2);
        // Lines are 1-indexed in the output
        assert_eq!(result[0].call_sites[0].start_line, 13); // 12 + 1
        assert_eq!(result[0].call_sites[1].start_line, 15); // 14 + 1
    }

    #[test]
    fn test_convert_incoming_calls_empty() {
        let result = convert_incoming_calls(vec![], Path::new("/src"));
        assert!(result.is_empty());
    }

    #[test]
    fn test_convert_outgoing_calls() {
        let outgoing =
            vec![CallHierarchyOutgoingCall { to: make_item("callee_fn", 20), from_ranges: vec![make_range(5, 10)] }];

        let result = convert_outgoing_calls("/src/caller.rs", Path::new("/src"), outgoing);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item.name, "callee_fn");
        assert_eq!(result[0].call_sites.len(), 1);
        assert_eq!(result[0].call_sites[0].file_path, "/src/caller.rs");
        assert_eq!(result[0].call_sites[0].start_line, 6); // 5 + 1
    }

    #[test]
    fn test_convert_outgoing_calls_empty() {
        let result = convert_outgoing_calls("/src/caller.rs", Path::new("/src"), vec![]);
        assert!(result.is_empty());
    }
}
