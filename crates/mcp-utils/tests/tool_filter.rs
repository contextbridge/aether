use mcp_utils::gateway::{ToolAnnotationMatcher, ToolFilter, ToolMatcher};
use mcp_utils::model::{Tool, ToolAnnotations};
use std::sync::Arc;

#[test]
fn empty_filter_allows_all_tools() {
    assert_eq!(allowed(&ToolFilter::default(), [tool("bash"), tool("read_file")]), ["bash", "read_file"]);
}

#[test]
fn allow_keeps_only_matching_tools() {
    let filter = ToolFilter { allow: vec![ToolMatcher::name("read_file"), ToolMatcher::name("grep")], deny: vec![] };
    assert_eq!(allowed(&filter, [tool("bash"), tool("read_file"), tool("grep")]), ["read_file", "grep"]);
}

#[test]
fn deny_removes_matching_tools() {
    let filter = ToolFilter { allow: vec![], deny: vec![ToolMatcher::name("bash")] };
    assert_eq!(allowed(&filter, [tool("bash"), tool("read_file")]), ["read_file"]);
}

#[test]
fn wildcard_matching() {
    let filter = ToolFilter { allow: vec![ToolMatcher::name("coding__*")], deny: vec![] };
    let tools = [tool("coding__grep"), tool("coding__read_file"), tool("plugins__bash")];
    assert_eq!(allowed(&filter, tools), ["coding__grep", "coding__read_file"]);
}

#[test]
fn combined_allow_and_deny() {
    let filter =
        ToolFilter { allow: vec![ToolMatcher::name("coding__*")], deny: vec![ToolMatcher::name("coding__write_file")] };
    let tools = [tool("coding__grep"), tool("coding__write_file"), tool("coding__read_file"), tool("plugins__bash")];
    assert_eq!(allowed(&filter, tools), ["coding__grep", "coding__read_file"]);
}

#[test]
fn annotation_allow_matches_present_values() {
    let filter = ToolFilter { allow: vec![ToolMatcher::read_only()], deny: vec![] };
    let tools = [
        tool("unknown"),
        annotated("read", ToolAnnotations::new().read_only(true)),
        annotated("write", ToolAnnotations::new().read_only(false)),
    ];
    assert_eq!(allowed(&filter, tools), ["read"]);
}

#[test]
fn deny_annotation_removes_destructive_tools() {
    let filter = ToolFilter {
        allow: vec![],
        deny: vec![annotation(ToolAnnotationMatcher { destructive: Some(true), ..ToolAnnotationMatcher::default() })],
    };
    let tools = [
        tool("unknown"),
        annotated("safe_update", ToolAnnotations::new().read_only(false).destructive(false)),
        annotated("delete", ToolAnnotations::new().destructive(true)),
    ];
    assert_eq!(allowed(&filter, tools), ["unknown", "safe_update"]);
}

#[test]
fn annotation_matchers_do_not_match_missing_fields() {
    let filter = ToolFilter {
        allow: vec![],
        deny: vec![
            annotation(ToolAnnotationMatcher { destructive: Some(true), ..ToolAnnotationMatcher::default() }),
            annotation(ToolAnnotationMatcher { open_world: Some(true), ..ToolAnnotationMatcher::default() }),
            annotation(ToolAnnotationMatcher { idempotent: Some(false), ..ToolAnnotationMatcher::default() }),
            annotation(ToolAnnotationMatcher { read_only: Some(false), ..ToolAnnotationMatcher::default() }),
        ],
    };
    assert_eq!(allowed(&filter, [tool("unknown")]), ["unknown"]);
}

#[test]
fn annotation_matchers_do_not_infer_fields_from_read_only_hint() {
    let filter = ToolFilter {
        allow: vec![annotation(ToolAnnotationMatcher { destructive: Some(false), ..ToolAnnotationMatcher::default() })],
        deny: vec![],
    };
    assert!(allowed(&filter, [annotated("read", ToolAnnotations::new().read_only(true))]).is_empty());
}

#[test]
fn deny_wins_over_allow() {
    let filter =
        ToolFilter { allow: vec![ToolMatcher::read_only()], deny: vec![ToolMatcher::name("coding__read_file")] };
    assert!(allowed(&filter, [annotated("coding__read_file", ToolAnnotations::new().read_only(true))]).is_empty());
}

#[test]
fn mixed_allow_entries_are_ored() {
    let filter = ToolFilter { allow: vec![ToolMatcher::read_only(), ToolMatcher::name("review__*")], deny: vec![] };
    let tools = [
        annotated("coding__grep", ToolAnnotations::new().read_only(true)),
        tool("review__review_artifact"),
        tool("coding__bash"),
    ];
    assert_eq!(allowed(&filter, tools), ["coding__grep", "review__review_artifact"]);
}

#[test]
fn empty_annotation_matcher_matches_nothing() {
    let filter = ToolFilter { allow: vec![annotation(ToolAnnotationMatcher::default())], deny: vec![] };
    assert!(allowed(&filter, [annotated("coding__grep", ToolAnnotations::new().read_only(true))]).is_empty());
}

#[test]
fn exact_name_match_is_not_a_prefix_match() {
    let filter = ToolFilter { allow: vec![ToolMatcher::name("bash")], deny: vec![] };
    assert_eq!(allowed(&filter, [tool("bash"), tool("bash_extended")]), ["bash"]);
}

#[test]
fn tool_matcher_uses_exact_and_trailing_wildcard_names() {
    let exact = ToolMatcher::name("foo");
    let wildcard = ToolMatcher::name("foo*");
    assert!(exact.matches(&tool("foo")));
    assert!(!exact.matches(&tool("foobar")));
    assert!(wildcard.matches(&tool("foobar")));
    assert!(wildcard.matches(&tool("foo")));
    assert!(!wildcard.matches(&tool("bar")));
}

fn allowed(filter: &ToolFilter, tools: impl IntoIterator<Item = Tool>) -> Vec<String> {
    tools.into_iter().filter(|tool| filter.is_tool_allowed(tool)).map(|tool| tool.name.to_string()).collect()
}

fn tool(name: &str) -> Tool {
    Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()))
}

fn annotated(name: &str, annotations: ToolAnnotations) -> Tool {
    tool(name).with_annotations(annotations)
}

fn annotation(matcher: ToolAnnotationMatcher) -> ToolMatcher {
    ToolMatcher::annotations(matcher)
}
