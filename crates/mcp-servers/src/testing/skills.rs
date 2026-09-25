//! Builder for skill and flat prompt file fixtures used by skills tests.

pub const SKILL_FILENAME: &str = "SKILL.md";

/// Builds the content of a `SKILL.md` or flat prompt file (`<name>.md`).
///
/// Only fields a test sets are written to the frontmatter. Skills default to
/// `agent-invocable: true` since that is what almost every fixture wants;
/// override with [`SkillBuilder::agent_invocable`] when not.
#[derive(Clone)]
pub struct SkillBuilder {
    name: Option<String>,
    description: Option<String>,
    user_invocable: Option<bool>,
    agent_invocable: Option<bool>,
    agent_authored: Option<bool>,
    tags: Vec<String>,
    read_triggers: Vec<String>,
    body: String,
}

impl Default for SkillBuilder {
    fn default() -> Self {
        Self {
            name: None,
            description: None,
            user_invocable: None,
            agent_invocable: Some(true),
            agent_authored: None,
            tags: Vec::new(),
            read_triggers: Vec::new(),
            body: String::new(),
        }
    }
}

impl SkillBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the frontmatter `name`, overriding the name derived from the directory or file stem.
    pub fn name(mut self, name: &str) -> Self {
        self.name = Some(name.to_string());
        self
    }

    pub fn description(mut self, description: &str) -> Self {
        self.description = Some(description.to_string());
        self
    }

    pub fn user_invocable(mut self, user_invocable: bool) -> Self {
        self.user_invocable = Some(user_invocable);
        self
    }

    pub fn agent_invocable(mut self, agent_invocable: bool) -> Self {
        self.agent_invocable = Some(agent_invocable);
        self
    }

    pub fn agent_authored(mut self, agent_authored: bool) -> Self {
        self.agent_authored = Some(agent_authored);
        self
    }

    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_string());
        self
    }

    /// Adds a `triggers.read` glob pattern for automatic activation.
    pub fn read_trigger(mut self, pattern: &str) -> Self {
        self.read_triggers.push(pattern.to_string());
        self
    }

    /// Sets the markdown body written below the frontmatter.
    pub fn body(mut self, body: &str) -> Self {
        self.body = body.to_string();
        self
    }

    /// Renders the frontmatter and body, e.g. for [`TestWorkspace::file`](super::TestWorkspace::file).
    pub fn content(&self) -> String {
        let mut frontmatter = Vec::new();
        if let Some(description) = &self.description {
            frontmatter.push(format!("description: {description}"));
        }
        if let Some(name) = &self.name {
            frontmatter.push(format!("name: {name}"));
        }
        if let Some(user_invocable) = self.user_invocable {
            frontmatter.push(format!("user-invocable: {user_invocable}"));
        }
        if let Some(agent_invocable) = self.agent_invocable {
            frontmatter.push(format!("agent-invocable: {agent_invocable}"));
        }
        if let Some(agent_authored) = self.agent_authored {
            frontmatter.push(format!("agent_authored: {agent_authored}"));
        }
        if !self.tags.is_empty() {
            frontmatter.push("tags:".to_string());
            frontmatter.extend(self.tags.iter().map(|tag| format!("  - {tag}")));
        }
        if !self.read_triggers.is_empty() {
            frontmatter.push("triggers:".to_string());
            frontmatter.push("  read:".to_string());
            frontmatter.extend(self.read_triggers.iter().map(|pattern| format!("    - \"{pattern}\"")));
        }

        if frontmatter.is_empty() {
            return self.body.clone();
        }

        let mut content = format!("---\n{}\n---", frontmatter.join("\n"));
        if !self.body.is_empty() {
            content.push('\n');
            content.push_str(&self.body);
        }
        content
    }
}

/// A [`SkillBuilder`] with the common defaults, e.g.
/// `skill().description("Rust rules").read_trigger("**/*.rs")`.
pub fn skill() -> SkillBuilder {
    SkillBuilder::new()
}
