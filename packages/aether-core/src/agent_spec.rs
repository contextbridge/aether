//! Agent specification types for authored agent definitions.
//!
//! `AgentSpec` is the canonical abstraction for authored agent definitions across the stack.
//! It represents a resolved runtime type, not a raw settings DTO.

use crate::core::Prompt;
use llm::{LlmModel, ProviderConnectionOverrides, ReasoningEffort, ToolDefinition};
use mcp_utils::client::McpConfig;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum McpConfigSource {
    File { path: PathBuf, proxy: bool },
    Json(String),
    Inline(McpConfig),
}

impl McpConfigSource {
    pub fn file(path: PathBuf, proxy: bool) -> Self {
        Self::File { path, proxy }
    }

    pub fn direct(path: PathBuf) -> Self {
        Self::file(path, false)
    }

    pub fn proxied(path: PathBuf) -> Self {
        Self::file(path, true)
    }
}

/// A resolved agent specification ready for runtime use.
///
/// This type is produced by validating and resolving authored agent configuration.
/// All validation happens before constructing these runtime types.
#[derive(Debug, Clone)]
pub struct AgentSpec {
    /// The canonical lookup key for this agent.
    pub name: String,
    /// Human-readable description of this agent's purpose.
    pub description: String,
    /// The validated model spec to use for this agent.
    ///
    /// This is stored as a canonical string so authored settings can represent
    /// both single models (`provider:model`) and alloy specs
    /// (`provider1:model1,provider2:model2`).
    pub model: String,
    /// Optional reasoning effort level for models that support it.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Effective context window in tokens for this agent.
    pub context_window: Option<u32>,
    /// The prompt stack for this agent.
    pub prompts: Vec<Prompt>,
    /// Provider connection overrides keyed by model provider name.
    pub provider_connections: ProviderConnectionOverrides,
    /// Resolved MCP config sources for this agent, applied in order.
    ///
    /// Direct server name collisions use last-source-wins semantics. Proxy-enabled
    /// file sources are merged into a single runtime tool proxy.
    pub mcp_config_sources: Vec<McpConfigSource>,
    /// How this agent can be invoked.
    pub exposure: AgentSpecExposure,
    /// Tool filter for restricting which MCP tools this agent can use.
    pub tools: ToolFilter,
}

impl AgentSpec {
    /// Create a default (no-mode) agent spec with the provided prompts.
    pub fn default_spec(model: &LlmModel, reasoning_effort: Option<ReasoningEffort>, prompts: Vec<Prompt>) -> Self {
        Self {
            name: "__default__".to_string(),
            description: "Default agent".to_string(),
            model: model.to_string(),
            reasoning_effort,
            context_window: None,
            prompts,
            provider_connections: ProviderConnectionOverrides::default(),
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::none(),
            tools: ToolFilter::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ToolMatcher {
    Name(String),
    Annotations(ToolAnnotationMatcher),
}

impl ToolMatcher {
    pub fn name(pattern: impl Into<String>) -> Self {
        Self::Name(pattern.into())
    }

    pub fn read_only() -> Self {
        Self::Annotations(ToolAnnotationMatcher { read_only: Some(true), ..ToolAnnotationMatcher::default() })
    }

    pub fn annotations(matcher: ToolAnnotationMatcher) -> Self {
        Self::Annotations(matcher)
    }

    pub fn matches(&self, tool: &ToolDefinition) -> bool {
        match self {
            Self::Name(pattern) => matches_pattern(pattern, &tool.name),
            Self::Annotations(matcher) => matcher.matches(tool),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolAnnotationMatcher {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world: Option<bool>,
}

impl ToolAnnotationMatcher {
    pub fn matches(&self, tool: &ToolDefinition) -> bool {
        let Some(annotations) = tool.annotations.as_ref() else {
            return false;
        };
        let pairs = [
            (self.read_only, annotations.read_only_hint),
            (self.destructive, annotations.destructive_hint),
            (self.idempotent, annotations.idempotent_hint),
            (self.open_world, annotations.open_world_hint),
        ];
        if pairs.iter().all(|(field, _)| field.is_none()) {
            return false;
        }
        pairs.iter().all(|(field, hint)| field.is_none_or(|value| *hint == Some(value)))
    }
}

/// Filter for restricting which tools an agent can use.
///
/// Supports `allow` (allowlist) and `deny` (blocklist) with name patterns and MCP annotation matchers.
/// If both are set, allow is applied first, then deny removes from the result.
/// An empty filter (the default) allows all tools.
#[doc = ""]
#[doc = include_str!("docs/tool_filter.md")]
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolFilter {
    /// If non-empty, only tools matching these patterns or annotations are allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<ToolMatcher>,
    /// Tools matching these patterns or annotations are removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<ToolMatcher>,
}

impl ToolFilter {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Apply this filter to a list of tool definitions.
    pub fn apply(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        tools.into_iter().filter(|tool| self.is_tool_allowed(tool)).collect()
    }

    pub fn is_tool_allowed(&self, tool: &ToolDefinition) -> bool {
        let allowed = self.allow.is_empty() || self.allow.iter().any(|matcher| matcher.matches(tool));
        let denied = self.deny.iter().any(|matcher| matcher.matches(tool));
        allowed && !denied
    }
}

/// Match a pattern against a name, supporting a trailing `*` wildcard.
fn matches_pattern(pattern: &str, name: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') { name.starts_with(prefix) } else { pattern == name }
}

/// Defines how an agent can be invoked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AgentSpecExposure {
    /// Whether this agent can be invoked by users (e.g., as an ACP mode).
    pub user_invocable: bool,
    /// Whether this agent can be invoked by other agents (e.g., as a sub-agent).
    pub agent_invocable: bool,
}

impl AgentSpecExposure {
    /// Create an exposure that is neither user nor agent invocable.
    ///
    /// Used internally for synthesized default specs (e.g., no-mode sessions).
    /// Not intended for authored agent definitions — all authored agents must
    /// have at least one invocation surface.
    pub fn none() -> Self {
        Self { user_invocable: false, agent_invocable: false }
    }

    /// Create an exposure that is only user invocable.
    pub fn user_only() -> Self {
        Self { user_invocable: true, agent_invocable: false }
    }

    /// Create an exposure that is only agent invocable.
    pub fn agent_only() -> Self {
        Self { user_invocable: false, agent_invocable: true }
    }

    /// Create an exposure that is both user and agent invocable.
    pub fn both() -> Self {
        Self { user_invocable: true, agent_invocable: true }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm::ToolAnnotations;

    #[test]
    fn default_spec_has_expected_fields() {
        let model: LlmModel = "anthropic:claude-sonnet-4-5".parse().unwrap();
        let prompts = vec![Prompt::file(PathBuf::from("/tmp/BASE.md"), PathBuf::from("/tmp"))];
        let spec = AgentSpec::default_spec(&model, None, prompts.clone());

        assert_eq!(spec.name, "__default__");
        assert_eq!(spec.description, "Default agent");
        assert_eq!(spec.model, model.to_string());
        assert!(spec.reasoning_effort.is_none());
        assert_eq!(spec.prompts.len(), 1);
        assert!(spec.mcp_config_sources.is_empty());
        assert_eq!(spec.exposure, AgentSpecExposure::none());
    }

    fn make_tool(name: &str) -> ToolDefinition {
        ToolDefinition::new(name, "", "")
    }

    fn make_annotated_tool(name: &str, annotations: ToolAnnotations) -> ToolDefinition {
        ToolDefinition::new(name, "", "").with_annotations(annotations)
    }

    #[test]
    fn empty_filter_allows_all_tools() {
        let filter = ToolFilter::default();
        let tools = vec![make_tool("bash"), make_tool("read_file")];
        let result = filter.apply(tools);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn allow_keeps_only_matching_tools() {
        let filter =
            ToolFilter { allow: vec![ToolMatcher::name("read_file"), ToolMatcher::name("grep")], deny: vec![] };
        let tools = vec![make_tool("bash"), make_tool("read_file"), make_tool("grep")];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file", "grep"]);
    }

    #[test]
    fn deny_removes_matching_tools() {
        let filter = ToolFilter { allow: vec![], deny: vec![ToolMatcher::name("bash")] };
        let tools = vec![make_tool("bash"), make_tool("read_file")];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file"]);
    }

    #[test]
    fn wildcard_matching() {
        let filter = ToolFilter { allow: vec![ToolMatcher::name("coding__*")], deny: vec![] };
        let tools = vec![make_tool("coding__grep"), make_tool("coding__read_file"), make_tool("plugins__bash")];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["coding__grep", "coding__read_file"]);
    }

    #[test]
    fn combined_allow_and_deny() {
        let filter = ToolFilter {
            allow: vec![ToolMatcher::name("coding__*")],
            deny: vec![ToolMatcher::name("coding__write_file")],
        };
        let tools = vec![
            make_tool("coding__grep"),
            make_tool("coding__write_file"),
            make_tool("coding__read_file"),
            make_tool("plugins__bash"),
        ];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["coding__grep", "coding__read_file"]);
    }

    #[test]
    fn annotation_allow_matches_present_values() {
        let filter = ToolFilter { allow: vec![ToolMatcher::read_only()], deny: vec![] };
        let tools = vec![
            make_tool("unknown"),
            make_annotated_tool("read", ToolAnnotations { read_only_hint: Some(true), ..ToolAnnotations::default() }),
            make_annotated_tool("write", ToolAnnotations { read_only_hint: Some(false), ..ToolAnnotations::default() }),
        ];
        let names: Vec<_> = filter.apply(tools).into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, vec!["read"]);
    }

    #[test]
    fn deny_annotation_removes_destructive_tools() {
        let filter = ToolFilter {
            allow: vec![],
            deny: vec![ToolMatcher::annotations(ToolAnnotationMatcher {
                destructive: Some(true),
                ..ToolAnnotationMatcher::default()
            })],
        };
        let tools = vec![
            make_tool("unknown"),
            make_annotated_tool(
                "safe_update",
                ToolAnnotations {
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    ..ToolAnnotations::default()
                },
            ),
        ];
        let names: Vec<_> = filter.apply(tools).into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, vec!["unknown", "safe_update"]);
    }

    #[test]
    fn annotation_matchers_do_not_match_missing_fields() {
        let filter = ToolFilter {
            allow: vec![],
            deny: vec![
                ToolMatcher::annotations(ToolAnnotationMatcher {
                    destructive: Some(true),
                    ..ToolAnnotationMatcher::default()
                }),
                ToolMatcher::annotations(ToolAnnotationMatcher {
                    open_world: Some(true),
                    ..ToolAnnotationMatcher::default()
                }),
                ToolMatcher::annotations(ToolAnnotationMatcher {
                    idempotent: Some(false),
                    ..ToolAnnotationMatcher::default()
                }),
                ToolMatcher::annotations(ToolAnnotationMatcher {
                    read_only: Some(false),
                    ..ToolAnnotationMatcher::default()
                }),
            ],
        };
        let tools = vec![make_tool("unknown")];
        let names: Vec<_> = filter.apply(tools).into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, vec!["unknown"]);
    }

    #[test]
    fn annotation_matchers_do_not_infer_fields_from_read_only_hint() {
        let filter = ToolFilter {
            allow: vec![ToolMatcher::annotations(ToolAnnotationMatcher {
                destructive: Some(false),
                ..ToolAnnotationMatcher::default()
            })],
            deny: vec![],
        };
        let tools = vec![make_annotated_tool("read", ToolAnnotations::read_only())];
        assert!(filter.apply(tools).is_empty());
    }

    #[test]
    fn deny_wins_over_allow() {
        let filter =
            ToolFilter { allow: vec![ToolMatcher::read_only()], deny: vec![ToolMatcher::name("coding__read_file")] };
        let tools = vec![make_annotated_tool(
            "coding__read_file",
            ToolAnnotations { read_only_hint: Some(true), ..ToolAnnotations::default() },
        )];
        assert!(filter.apply(tools).is_empty());
    }

    #[test]
    fn mixed_allow_entries_are_ored() {
        let filter = ToolFilter { allow: vec![ToolMatcher::read_only(), ToolMatcher::name("plan__*")], deny: vec![] };
        let tools = vec![
            make_annotated_tool(
                "coding__grep",
                ToolAnnotations { read_only_hint: Some(true), ..ToolAnnotations::default() },
            ),
            make_tool("plan__write_plan"),
            make_tool("coding__bash"),
        ];
        let names: Vec<_> = filter.apply(tools).into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, vec!["coding__grep", "plan__write_plan"]);
    }

    #[test]
    fn empty_annotation_matcher_matches_nothing() {
        let filter =
            ToolFilter { allow: vec![ToolMatcher::annotations(ToolAnnotationMatcher::default())], deny: vec![] };
        let tools = vec![make_annotated_tool(
            "coding__grep",
            ToolAnnotations { read_only_hint: Some(true), ..ToolAnnotations::default() },
        )];
        assert!(filter.apply(tools).is_empty());
    }

    #[test]
    fn exact_name_match_is_not_a_prefix_match() {
        let filter = ToolFilter { allow: vec![ToolMatcher::name("bash")], deny: vec![] };
        let names: Vec<_> =
            filter.apply(vec![make_tool("bash"), make_tool("bash_extended")]).into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["bash"]);
    }

    #[test]
    fn matches_pattern_exact_and_wildcard() {
        assert!(matches_pattern("foo", "foo"));
        assert!(!matches_pattern("foo", "foobar"));
        assert!(matches_pattern("foo*", "foobar"));
        assert!(matches_pattern("foo*", "foo"));
        assert!(!matches_pattern("bar*", "foo"));
    }
}
