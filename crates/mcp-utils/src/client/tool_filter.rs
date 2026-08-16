use llm::ToolDefinition;
use utils::matches_name_pattern;

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
            Self::Name(pattern) => matches_name_pattern(pattern, &tool.name),
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

/// Filter for restricting which MCP tools an agent may discover and execute.
///
/// Supports `allow` (allowlist) and `deny` (blocklist) with name patterns and MCP annotation matchers.
/// If both are set, allow is applied first, then deny removes from the result.
/// An empty filter (the default) allows all tools.
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

    pub fn apply(&self, tools: Vec<ToolDefinition>) -> Vec<ToolDefinition> {
        tools.into_iter().filter(|tool| self.is_tool_allowed(tool)).collect()
    }

    pub fn is_tool_allowed(&self, tool: &ToolDefinition) -> bool {
        let allowed = self.allow.is_empty() || self.allow.iter().any(|matcher| matcher.matches(tool));
        let denied = self.deny.iter().any(|matcher| matcher.matches(tool));
        allowed && !denied
    }
}
