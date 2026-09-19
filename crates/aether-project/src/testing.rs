//! Shared fixtures and fake builders for exercising `aether-project` in tests.
//!
//! Compiled for the crate's own tests and for any consumer that enables the
//! `testing` feature. Prefer these over hand-rolling per-suite fixtures.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentConfig, PromptSource, SKILL_FILENAME};
use aether_core::agent_spec::{AgentSpec, AgentSpecExposure};
use llm::{ModelSettings, ProviderConnectionOverrides};
use mcp_utils::client::ToolFilter;

/// The model id every shared agent fixture resolves to.
pub const DEFAULT_MODEL: &str = "anthropic:claude-sonnet-4-5";

/// A temporary project directory for tests that need on-disk settings, prompts, or skills.
pub fn project() -> TestProject {
    TestProject { root: temp_dir("project") }
}

/// A temporary user home directory for tests that exercise `AETHER_HOME` lookups.
pub fn home() -> TestHome {
    TestHome { root: temp_dir("home") }
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

/// A minimal [`AgentSpec`] with the given name and exposure.
pub fn fake_spec(name: &str, exposure: AgentSpecExposure) -> AgentSpec {
    AgentSpec {
        name: name.to_string(),
        description: format!("{name} agent"),
        model: DEFAULT_MODEL.to_string(),
        reasoning_effort: None,
        model_settings: ModelSettings::default(),
        context_window: None,
        prompts: vec![],
        provider_connections: ProviderConnectionOverrides::default(),
        mcp_config_sources: Vec::new(),
        exposure,
        tools: ToolFilter::default(),
    }
}

/// A temporary project directory whose files are set up fluently.
pub struct TestProject {
    root: TempDir,
}

impl TestProject {
    /// The project root path.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Writes `content` to `path` relative to the project root, creating parent directories.
    pub fn file(self, path: &str, content: &str) -> Self {
        self.write(path, content);
        self
    }

    /// Writes `content` to `<name>/SKILL.md`, creating the skill directory.
    pub fn skill(self, name: &str, content: &str) -> Self {
        self.file(&format!("{name}/{SKILL_FILENAME}"), content)
    }

    /// Writes a file after the fixture exists, e.g. to exercise a state transition.
    pub fn write(&self, path: &str, content: &str) {
        write_file(self.root.path(), path, content);
    }
}

/// A temporary home directory whose `.aether` contents are set up fluently.
pub struct TestHome {
    root: TempDir,
}

impl TestHome {
    /// The `.aether` directory inside this home.
    pub fn aether(&self) -> PathBuf {
        self.root.path().join(".aether")
    }

    /// The home root path.
    pub fn root(&self) -> &Path {
        self.root.path()
    }

    /// Writes `content` to `path` relative to the home root, creating parent directories.
    pub fn file(self, path: &str, content: &str) -> Self {
        write_file(self.root.path(), path, content);
        self
    }

    /// Writes user settings to `.aether/settings.json`.
    pub fn settings(self, json: &str) -> Self {
        self.file(".aether/settings.json", json)
    }
}

fn temp_dir(label: &str) -> TempDir {
    TempDir::new().unwrap_or_else(|error| panic!("failed to create temp {label} dir: {error}"))
}

fn write_file(root: &Path, path: &str, content: &str) {
    let full_path = root.join(path);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).expect("failed to create fixture parent directories");
    }

    fs::write(full_path, content).expect("failed to write fixture file");
}
