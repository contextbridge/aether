//! Agent specification types for authored agent definitions.
//!
//! `AgentSpec` is the canonical abstraction for authored agent definitions across the stack.
//! It represents a resolved runtime type, not a raw settings DTO.

use crate::core::Prompt;
use llm::{LlmModel, ReasoningEffort, ToolDefinition};
use mcp_utils::client::RawMcpConfig;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum McpConfigSource {
    File { path: PathBuf, proxy: bool },
    Json(String),
    Inline(RawMcpConfig),
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
    /// The prompt stack for this agent.
    ///
    /// For authored `AgentSpec`s resolved from settings, this contains authored prompt
    /// variants (e.g., `Prompt::PromptGlobs`). Prompt files may include
    /// `` !`<shell command>` `` markers which are replaced by the trimmed stdout of
    /// the command at prompt-load time. `Prompt::McpInstructions` is added separately
    /// during agent construction.
    pub prompts: Vec<Prompt>,
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
            prompts,
            mcp_config_sources: Vec::new(),
            exposure: AgentSpecExposure::none(),
            tools: ToolFilter::default(),
        }
    }
}

/// Filter for restricting which tools an agent can use.
///
/// Supports `allow` (allowlist) and `deny` (blocklist) with trailing `*` wildcards.
/// If both are set, allow is applied first, then deny removes from the result.
/// An empty filter (the default) allows all tools.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ToolFilter {
    /// If non-empty, only tools matching these patterns are allowed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    /// Tools matching these patterns are removed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

impl ToolFilter {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    /// Apply this filter to a list of tool definitions.
    pub fn apply(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        tools.into_iter().filter(|t| self.is_allowed(&t.name)).collect()
    }

    /// Check whether a tool name passes this filter.
    pub fn is_allowed(&self, tool_name: &str) -> bool {
        let allowed = self.allow.is_empty() || self.allow.iter().any(|p| matches_pattern(p, tool_name));
        allowed && !self.deny.iter().any(|p| matches_pattern(p, tool_name))
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

    #[test]
    fn default_spec_has_expected_fields() {
        let model: LlmModel = "anthropic:claude-sonnet-4-5".parse().unwrap();
        let prompts = vec![Prompt::from_globs(vec!["BASE.md".to_string()], PathBuf::from("/tmp"))];
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
        ToolDefinition { name: name.to_string(), description: String::new(), parameters: String::new(), server: None }
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
        let filter = ToolFilter { allow: vec!["read_file".to_string(), "grep".to_string()], deny: vec![] };
        let tools = vec![make_tool("bash"), make_tool("read_file"), make_tool("grep")];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file", "grep"]);
    }

    #[test]
    fn deny_removes_matching_tools() {
        let filter = ToolFilter { allow: vec![], deny: vec!["bash".to_string()] };
        let tools = vec![make_tool("bash"), make_tool("read_file")];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["read_file"]);
    }

    #[test]
    fn wildcard_matching() {
        let filter = ToolFilter { allow: vec!["coding__*".to_string()], deny: vec![] };
        let tools = vec![make_tool("coding__grep"), make_tool("coding__read_file"), make_tool("plugins__bash")];
        let result = filter.apply(tools);
        let names: Vec<_> = result.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["coding__grep", "coding__read_file"]);
    }

    #[test]
    fn combined_allow_and_deny() {
        let filter =
            { ToolFilter { allow: vec!["coding__*".to_string()], deny: vec!["coding__write_file".to_string()] } };
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
    fn is_allowed_exact_match() {
        let filter = ToolFilter { allow: vec!["bash".to_string()], deny: vec![] };
        assert!(filter.is_allowed("bash"));
        assert!(!filter.is_allowed("bash_extended"));
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
