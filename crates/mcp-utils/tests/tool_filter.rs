use mcp_utils::client::{ToolAnnotationMatcher, ToolFilter, ToolMatcher};
use rmcp::model::{Tool, ToolAnnotations};
use std::sync::Arc;

#[test]
fn empty_filter_allows_all_tools() {
    let filter = ToolFilter::default();
    assert_eq!(names(&filter, vec![tool("bash"), tool("read_file")]), ["bash", "read_file"]);
}

#[test]
fn allow_keeps_only_matching_tools() {
    let filter = ToolFilter { allow: vec![ToolMatcher::name("read_file"), ToolMatcher::name("grep")], deny: vec![] };
    assert_eq!(names(&filter, vec![tool("bash"), tool("read_file"), tool("grep")]), ["read_file", "grep"]);
}

#[test]
fn deny_removes_matching_tools() {
    let filter = ToolFilter { allow: vec![], deny: vec![ToolMatcher::name("bash")] };
    assert_eq!(names(&filter, vec![tool("bash"), tool("read_file")]), ["read_file"]);
}

#[test]
fn wildcard_matching() {
    let filter = ToolFilter { allow: vec![ToolMatcher::name("coding__*")], deny: vec![] };
    let tools = vec![tool("coding__grep"), tool("coding__read_file"), tool("plugins__bash")];
    assert_eq!(names(&filter, tools), ["coding__grep", "coding__read_file"]);
}

#[test]
fn combined_allow_and_deny() {
    let filter =
        ToolFilter { allow: vec![ToolMatcher::name("coding__*")], deny: vec![ToolMatcher::name("coding__write_file")] };
    let tools =
        vec![tool("coding__grep"), tool("coding__write_file"), tool("coding__read_file"), tool("plugins__bash")];
    assert_eq!(names(&filter, tools), ["coding__grep", "coding__read_file"]);
}

#[test]
fn annotation_allow_matches_present_values() {
    let filter = ToolFilter { allow: vec![ToolMatcher::read_only()], deny: vec![] };
    let tools = vec![
        tool("unknown"),
        tool("read").with_annotations(ToolAnnotations::new().read_only(true)),
        tool("write").with_annotations(ToolAnnotations::new().read_only(false)),
    ];
    assert_eq!(names(&filter, tools), ["read"]);
}

#[test]
fn deny_annotation_removes_destructive_tools() {
    let filter = ToolFilter {
        allow: vec![],
        deny: vec![annotations(ToolAnnotationMatcher { destructive: Some(true), ..Default::default() })],
    };
    let tools = vec![
        tool("unknown"),
        tool("safe_update").with_annotations(ToolAnnotations::new().read_only(false).destructive(false)),
    ];
    assert_eq!(names(&filter, tools), ["unknown", "safe_update"]);
}

#[test]
fn annotation_matchers_do_not_match_missing_fields() {
    let filter = ToolFilter {
        allow: vec![],
        deny: vec![
            annotations(ToolAnnotationMatcher { destructive: Some(true), ..Default::default() }),
            annotations(ToolAnnotationMatcher { open_world: Some(true), ..Default::default() }),
            annotations(ToolAnnotationMatcher { idempotent: Some(false), ..Default::default() }),
            annotations(ToolAnnotationMatcher { read_only: Some(false), ..Default::default() }),
        ],
    };
    assert_eq!(names(&filter, vec![tool("unknown")]), ["unknown"]);
}

#[test]
fn annotation_matchers_do_not_infer_fields_from_read_only_hint() {
    let filter = ToolFilter {
        allow: vec![annotations(ToolAnnotationMatcher { destructive: Some(false), ..Default::default() })],
        deny: vec![],
    };
    let tools = vec![tool("read").with_annotations(ToolAnnotations::new().read_only(true))];
    assert!(names(&filter, tools).is_empty());
}

#[test]
fn deny_wins_over_allow() {
    let filter =
        ToolFilter { allow: vec![ToolMatcher::read_only()], deny: vec![ToolMatcher::name("coding__read_file")] };
    let tools = vec![tool("coding__read_file").with_annotations(ToolAnnotations::new().read_only(true))];
    assert!(names(&filter, tools).is_empty());
}

#[test]
fn mixed_allow_entries_are_ored() {
    let filter = ToolFilter { allow: vec![ToolMatcher::read_only(), ToolMatcher::name("review__*")], deny: vec![] };
    let tools = vec![
        tool("coding__grep").with_annotations(ToolAnnotations::new().read_only(true)),
        tool("review__review_artifact"),
        tool("coding__bash"),
    ];
    assert_eq!(names(&filter, tools), ["coding__grep", "review__review_artifact"]);
}

#[test]
fn empty_annotation_matcher_matches_nothing() {
    let filter = ToolFilter { allow: vec![annotations(ToolAnnotationMatcher::default())], deny: vec![] };
    let tools = vec![tool("coding__grep").with_annotations(ToolAnnotations::new().read_only(true))];
    assert!(names(&filter, tools).is_empty());
}

#[test]
fn exact_name_match_is_not_a_prefix_match() {
    let filter = ToolFilter { allow: vec![ToolMatcher::name("bash")], deny: vec![] };
    assert_eq!(names(&filter, vec![tool("bash"), tool("bash_extended")]), ["bash"]);
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

fn tool(name: &str) -> Tool {
    Tool::new(name.to_string(), "", Arc::new(serde_json::Map::new()))
}

fn annotations(matcher: ToolAnnotationMatcher) -> ToolMatcher {
    ToolMatcher::annotations(matcher)
}

fn names(filter: &ToolFilter, tools: Vec<Tool>) -> Vec<String> {
    filter.apply(tools).into_iter().map(|tool| tool.name.into_owned()).collect()
}
