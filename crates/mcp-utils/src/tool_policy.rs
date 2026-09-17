use utils::matches_name_pattern;

/// The definition fields used by tool policies, independent of client/model types.
pub trait ToolPolicyTarget {
    fn policy_name(&self) -> &str;
    fn policy_annotations(&self) -> Option<[Option<bool>; 4]>;
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ToolMatcher {
    Name(String),
    Annotations(ToolAnnotationMatcher),
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

/// Filter for restricting which MCP tools an agent may discover and execute.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolFilter {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<ToolMatcher>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<ToolMatcher>,
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

    pub fn matches(&self, tool: &impl ToolPolicyTarget) -> bool {
        self.matches_named(tool.policy_name(), tool)
    }

    fn matches_named(&self, name: &str, tool: &impl ToolPolicyTarget) -> bool {
        match self {
            Self::Name(pattern) => matches_name_pattern(pattern, name),
            Self::Annotations(matcher) => matcher.matches(tool),
        }
    }
}

impl ToolAnnotationMatcher {
    pub fn matches(&self, tool: &impl ToolPolicyTarget) -> bool {
        let Some(hints) = tool.policy_annotations() else { return false };
        let fields = [self.read_only, self.destructive, self.idempotent, self.open_world];
        fields.iter().any(Option::is_some)
            && fields.iter().zip(hints).all(|(field, hint)| field.is_none_or(|value| hint == Some(value)))
    }
}

impl ToolFilter {
    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.deny.is_empty()
    }

    pub fn apply<T: ToolPolicyTarget>(&self, tools: Vec<T>) -> Vec<T> {
        tools.into_iter().filter(|tool| self.is_tool_allowed(tool)).collect()
    }

    pub fn is_tool_allowed(&self, tool: &impl ToolPolicyTarget) -> bool {
        self.allows_named(tool.policy_name(), tool)
    }

    /// Evaluate a naming boundary without copying or reducing the tool definition.
    pub fn allows_named(&self, name: &str, tool: &impl ToolPolicyTarget) -> bool {
        let allowed = self.allow.is_empty() || self.allow.iter().any(|matcher| matcher.matches_named(name, tool));
        let denied = self.deny.iter().any(|matcher| matcher.matches_named(name, tool));
        allowed && !denied
    }
}

impl ToolPolicyTarget for rmcp::model::Tool {
    fn policy_name(&self) -> &str {
        &self.name
    }

    fn policy_annotations(&self) -> Option<[Option<bool>; 4]> {
        self.annotations.as_ref().map(|a| [a.read_only_hint, a.destructive_hint, a.idempotent_hint, a.open_world_hint])
    }
}

#[cfg(feature = "client")]
impl ToolPolicyTarget for llm::ToolDefinition {
    fn policy_name(&self) -> &str {
        &self.name
    }

    fn policy_annotations(&self) -> Option<[Option<bool>; 4]> {
        self.annotations.as_ref().map(|a| [a.read_only_hint, a.destructive_hint, a.idempotent_hint, a.open_world_hint])
    }
}
