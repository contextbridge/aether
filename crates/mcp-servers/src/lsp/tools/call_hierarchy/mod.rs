//! LSP call hierarchy tool
//!
//! This module provides call hierarchy operations:
//! - incoming: Find functions/methods that call the given item
//! - outgoing: Find functions/methods that the given item calls

use lsp_types::CallHierarchyItem;
use schemars::JsonSchema;
use serde::Serialize;

use crate::lsp::common::{LocationResult, uri_to_path};
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
    /// The range of the entire symbol
    pub range: LocationResult,
    /// The range of the symbol name
    pub selection_range: LocationResult,
}

impl From<CallHierarchyItem> for CallHierarchyItemResult {
    fn from(item: CallHierarchyItem) -> Self {
        let file_path = uri_to_path(&item.uri);
        let range = LocationResult::from_range(file_path.clone(), &item.range);
        let selection_range = LocationResult::from_range(file_path.clone(), &item.selection_range);
        Self {
            name: item.name,
            kind: symbol_kind_to_string(item.kind).to_string(),
            detail: item.detail,
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
    /// The locations where calls occur
    pub call_sites: Vec<LocationResult>,
}

/// Convert LSP incoming calls to serializable `CallSiteResult`s.
pub fn convert_incoming_calls(incoming: Vec<lsp_types::CallHierarchyIncomingCall>) -> Vec<CallSiteResult> {
    incoming.into_iter().map(|call| convert_call(call.from, &call.from_ranges)).collect()
}

/// Convert LSP outgoing calls to serializable `CallSiteResult`s.
pub fn convert_outgoing_calls(outgoing: Vec<lsp_types::CallHierarchyOutgoingCall>) -> Vec<CallSiteResult> {
    outgoing.into_iter().map(|call| convert_call(call.to, &call.from_ranges)).collect()
}

/// Shared conversion for both incoming and outgoing calls.
fn convert_call(item: CallHierarchyItem, from_ranges: &[lsp_types::Range]) -> CallSiteResult {
    let file_path = uri_to_path(&item.uri);
    let call_sites = from_ranges.iter().map(|range| LocationResult::from_range(file_path.clone(), range)).collect();
    CallSiteResult { item: CallHierarchyItemResult::from(item), call_sites }
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

        let result = convert_incoming_calls(incoming);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item.name, "caller_fn");
        assert_eq!(result[0].call_sites.len(), 2);
        // Lines are 1-indexed in the output
        assert_eq!(result[0].call_sites[0].start_line, 13); // 12 + 1
        assert_eq!(result[0].call_sites[1].start_line, 15); // 14 + 1
    }

    #[test]
    fn test_convert_incoming_calls_empty() {
        let result = convert_incoming_calls(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_convert_outgoing_calls() {
        let outgoing =
            vec![CallHierarchyOutgoingCall { to: make_item("callee_fn", 20), from_ranges: vec![make_range(5, 10)] }];

        let result = convert_outgoing_calls(outgoing);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].item.name, "callee_fn");
        assert_eq!(result[0].call_sites.len(), 1);
        assert_eq!(result[0].call_sites[0].start_line, 6); // 5 + 1
    }

    #[test]
    fn test_convert_outgoing_calls_empty() {
        let result = convert_outgoing_calls(vec![]);
        assert!(result.is_empty());
    }
}
