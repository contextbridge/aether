//! Shared fixtures and fake builders for exercising `aether-project` in tests.
//!
//! Compiled for the crate's own tests and for any consumer that enables the
//! `testing` feature. Prefer these over hand-rolling per-suite fixtures.

use std::path::PathBuf;

use serde_json::{Value, json};

use crate::prompt_file::{PromptFile, PromptTriggers};
use crate::{AgentConfig, PromptSource};
use aether_core::agent_spec::{AgentSpec, AgentSpecExposure};
pub use utils::testing::TestWorkspace;

/// The model id every shared agent fixture resolves to.
pub const DEFAULT_MODEL: &str = "anthropic:claude-sonnet-4-5";

pub fn mcp_json(server: &str) -> String {
    json!({ "servers": { server: { "command": format!("{server}-server") } } }).to_string()
}

pub fn project() -> TestWorkspace {
    TestWorkspace::new()
}

pub fn home() -> TestWorkspace {
    TestWorkspace::new()
}

/// A minimal user-invocable [`AgentConfig`] as it would be parsed from `settings.json`.
pub fn settings_agent(name: &str, description: &str) -> AgentConfig {
    AgentConfig {
        name: name.to_string(),
        description: description.to_string(),
        model: DEFAULT_MODEL.to_string(),
        user_invocable: true,
        ..AgentConfig::default()
    }
}

/// A `settings.json` agent object for `name` and `description`: the
/// [`DEFAULT_MODEL`], user-invocable, with no prompts or MCPs.
pub fn agent_json(name: &str, description: &str) -> Value {
    json!({
        "name": name,
        "description": description,
        "model": DEFAULT_MODEL,
        "userInvocable": true,
    })
}

/// [`agent_json`] merged with `extra` fields, e.g.
/// `agent_json_with("planner", "Plans", json!({ "prompts": ["BASE.md"] }))`.
pub fn agent_json_with(name: &str, description: &str, extra: Value) -> Value {
    let Value::Object(extra) = extra else {
        panic!("extra agent fields must be a JSON object, got {extra}");
    };
    let mut agent = agent_json(name, description);
    agent.as_object_mut().expect("agent_json builds an object").extend(extra);
    agent
}

/// A user-invocable [`AgentConfig`] named `name` whose prompt is the file `PROMPT.md`.
pub fn agent_config(name: &str) -> AgentConfig {
    AgentConfig { prompts: vec![PromptSource::file("PROMPT.md")], ..settings_agent(name, &format!("{name} agent")) }
}

/// A [`PromptFile`] with valid defaults, built fluently for tests that exercise
/// skill parsing, writing, and rating behavior.
pub fn prompt_file(name: &str) -> PromptFileBuilder {
    PromptFileBuilder {
        file: PromptFile {
            name: name.to_string(),
            description: format!("{name} skill"),
            body: format!("# {name}\n\nSkill body."),
            path: PathBuf::new(),
            user_invocable: true,
            agent_invocable: true,
            argument_hint: None,
            tags: Vec::new(),
            triggers: PromptTriggers::default(),
            agent_authored: false,
            helpful: 0,
            harmful: 0,
        },
    }
}

/// A minimal [`AgentSpec`] with the given name and exposure.
pub fn fake_spec(name: &str, exposure: AgentSpecExposure) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        description: format!("{name} agent"),
        model: DEFAULT_MODEL.to_string(),
        exposure,
        ..AgentSpec::default()
    }
}

/// A fluent builder for test [`PromptFile`]s; see [`prompt_file`].
pub struct PromptFileBuilder {
    file: PromptFile,
}

impl PromptFileBuilder {
    /// The skill description shown to users selecting the skill.
    pub fn description(mut self, description: &str) -> Self {
        self.file.description = description.to_string();
        self
    }

    /// The skill body below the frontmatter.
    pub fn body(mut self, body: &str) -> Self {
        self.file.body = body.to_string();
        self
    }

    /// Whether the user can invoke the skill as a slash command.
    pub fn user_invocable(mut self, user_invocable: bool) -> Self {
        self.file.user_invocable = user_invocable;
        self
    }

    /// Whether the agent can invoke the skill on its own.
    pub fn agent_invocable(mut self, agent_invocable: bool) -> Self {
        self.file.agent_invocable = agent_invocable;
        self
    }

    /// The argument hint shown to users when invoking the skill.
    pub fn argument_hint(mut self, argument_hint: &str) -> Self {
        self.file.argument_hint = Some(argument_hint.to_string());
        self
    }

    /// The skill's tags.
    pub fn tags(mut self, tags: &[&str]) -> Self {
        self.file.tags = tags.iter().map(ToString::to_string).collect();
        self
    }

    /// Read-trigger glob patterns activating the skill when matching files are read.
    pub fn read_triggers(mut self, patterns: &[&str]) -> Self {
        self.file.triggers = PromptTriggers::new(patterns.iter().map(ToString::to_string).collect())
            .expect("read_triggers patterns must be valid globs");
        self
    }

    /// Whether the agent authored this skill.
    pub fn agent_authored(mut self, agent_authored: bool) -> Self {
        self.file.agent_authored = agent_authored;
        self
    }

    /// The helpful/harmful rating counts collected for this skill.
    pub fn ratings(mut self, helpful: u32, harmful: u32) -> Self {
        self.file.helpful = helpful;
        self.file.harmful = harmful;
        self
    }

    /// The finished [`PromptFile`].
    pub fn build(self) -> PromptFile {
        self.file
    }
}
