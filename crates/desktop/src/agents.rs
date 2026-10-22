//! The agents a project can start threads with: the built-in [`DEFAULT_AGENT`], and those the
//! user's and the project's `acp.json` files define.
//!
//! A file names each agent and says how to reach it, as `mcp.json` does for MCP servers:
//!
//! ```json
//! {
//!   "agents": {
//!     "claude": { "type": "stdio", "command": "npx", "args": ["-y", "@zed-industries/claude-code-acp"] },
//!     "devbox": { "type": "websocket", "url": "wss://devbox:8765", "headers": { "Authorization": "Bearer ${TOKEN}" } },
//!     "luna (VM)": {
//!       "type": "websocket",
//!       "launch": { "command": "./clankers/launch.sh", "args": ["luna-max"] },
//!       "endpoint": { "command": "./clankers/endpoint.sh", "args": ["${INSTANCE_ID}"] },
//!       "terminate": { "command": "./clankers/stop.sh", "args": ["${INSTANCE_ID}"] }
//!     }
//!   }
//! }
//! ```
//!
//! A WebSocket agent is reached at a fixed `url`, or wherever its `endpoint` command says, which
//! prints `{"url": "…", "headers": {…}}` and runs again for every connection, so that the
//! credentials it mints are fresh. An agent with a `launch` command runs on an instance it starts
//! the first time a thread connects: the command prints `{"id": "…"}`, which the agent's other
//! commands see as `${INSTANCE_ID}`, and its `terminate` command ends the instance when the thread
//! is stopped.
//!
//! `${WORKSPACE}` is the project's root, which commands run in, and other `${VAR}` references are
//! read from the environment. `$$` is a literal `$`, as a script given to `sh -c` needs for its
//! own variables.

use agent_client_protocol::AcpAgent;
use gpui::Context;
use serde::Deserialize;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use utils::fs::{JsonFileError, read_json};
use utils::variables::{VarError, Vars};

/// The name of the file agents are defined in, inside a project's `.aether` directory and the
/// user's desktop state directory.
pub const AGENTS_FILE: &str = "acp.json";

/// The agent every project offers, which saved sessions resume with.
pub const DEFAULT_AGENT: &str = "aether";

/// The agents each project offers, read from its files the first time they are asked for.
pub struct Agents {
    builtin: AgentCommand,
    user_file: Option<PathBuf>,
    catalogs: RefCell<HashMap<PathBuf, Rc<AgentCatalog>>>,
}

/// The agents one project offers, ordered by name, and the files that could not be read.
#[derive(Debug)]
pub struct AgentCatalog {
    agents: Vec<AgentProfile>,
    problems: Vec<JsonFileError>,
}

/// An agent a project offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub name: String,
    /// The root of the project that offers the agent, which its commands run in and
    /// `${WORKSPACE}` expands to.
    pub workspace: PathBuf,
    pub config: AgentConfig,
}

/// How to reach an agent, chosen by its `type`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentConfig {
    /// A process started in the thread's directory, speaking ACP over its stdin and stdout.
    Stdio(AgentCommand),
    /// A server speaking ACP over a WebSocket.
    WebSocket(WebSocketAgent),
}

/// A command line. `${VAR}` references in any of its parts are expanded before it runs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentCommand {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// An agent served over a WebSocket, with the commands that start and end what serves it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "WebSocketAgentFields")]
pub struct WebSocketAgent {
    pub endpoint: EndpointSource,
    pub launch: Option<AgentCommand>,
    pub terminate: Option<AgentCommand>,
}

/// Where a WebSocket agent is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointSource {
    /// The agent's own `url` and `headers`.
    Fixed(Endpoint),
    /// What the agent's `endpoint` command prints.
    Command(AgentCommand),
}

/// A WebSocket URL, and the HTTP headers the handshake sends.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Endpoint {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WebSocketAgentError {
    #[error("a websocket agent needs a `url` or an `endpoint` command")]
    MissingEndpoint,
    #[error("a websocket agent takes a `url` or an `endpoint` command, not both")]
    ConflictingEndpoints,
    #[error("`headers` go with a `url`, since an `endpoint` command prints its own")]
    HeadersWithoutUrl,
    #[error("a `terminate` command needs a `launch` command whose instance it ends")]
    TerminateWithoutLaunch,
}

/// Where the user defines agents for every project: `~/.aether/desktop/acp.json`, or `None`
/// without a home directory.
pub fn default_agents_file() -> Option<PathBuf> {
    utils::path::aether_home().map(|home| home.join("desktop").join(AGENTS_FILE))
}

/// Where `project` defines its agents.
pub fn project_agents_file(project: &Path) -> PathBuf {
    project.join(".aether").join(AGENTS_FILE)
}

impl Agents {
    /// Offers `builtin` as every project's [`DEFAULT_AGENT`], beside the agents `user_file` and
    /// each project's file define.
    pub fn new(builtin: AgentCommand, user_file: Option<PathBuf>) -> Self {
        Self { builtin, user_file, catalogs: RefCell::default() }
    }

    /// The agents `project` offers: the built-in one, then those of the user's file, then those
    /// of the project's file, each replacing any before it of the same name.
    pub fn catalog(&self, project: &Path) -> Rc<AgentCatalog> {
        self.catalogs.borrow_mut().entry(project.to_path_buf()).or_insert_with(|| Rc::new(self.read(project))).clone()
    }

    /// Forgets the catalogs read so far, so that each is read again when next asked for.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.catalogs.get_mut().clear();
        cx.notify();
    }

    fn read(&self, project: &Path) -> AgentCatalog {
        let mut configs = BTreeMap::from([(DEFAULT_AGENT.to_owned(), AgentConfig::Stdio(self.builtin.clone()))]);
        let mut problems = Vec::new();
        for file in self.user_file.iter().cloned().chain([project_agents_file(project)]) {
            match read_json::<AcpConfig>(&file) {
                Ok(config) => configs.extend(config.unwrap_or_default().agents),
                Err(error) => {
                    tracing::warn!(%error, "leaving out the agents of a file that cannot be read");
                    problems.push(error);
                }
            }
        }
        let agents = configs
            .into_iter()
            .map(|(name, config)| AgentProfile { name, workspace: project.to_path_buf(), config })
            .collect();
        AgentCatalog { agents, problems }
    }
}

impl AgentCatalog {
    pub fn agents(&self) -> &[AgentProfile] {
        &self.agents
    }

    pub fn get(&self, name: &str) -> Option<&AgentProfile> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    /// The [`DEFAULT_AGENT`], which every catalog has.
    pub fn default_agent(&self) -> &AgentProfile {
        self.get(DEFAULT_AGENT).expect("every catalog has the default agent")
    }

    /// The files whose agents are left out, because they could not be read.
    pub fn problems(&self) -> &[JsonFileError] {
        &self.problems
    }
}

impl AgentConfig {
    /// The command that starts the instance the agent runs on, if it runs on one.
    pub fn launch(&self) -> Option<&AgentCommand> {
        match self {
            Self::Stdio(_) => None,
            Self::WebSocket(agent) => agent.launch.as_ref(),
        }
    }

    /// The command that ends the instance the agent runs on, if it runs on one.
    pub fn terminate(&self) -> Option<&AgentCommand> {
        match self {
            Self::Stdio(_) => None,
            Self::WebSocket(agent) => agent.terminate.as_ref(),
        }
    }
}

impl AgentCommand {
    pub fn new(command: impl Into<String>) -> Self {
        Self { command: command.into(), args: Vec::new(), env: BTreeMap::new() }
    }

    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Reads a command line such as `aether acp`. Leading `NAME=value` words set environment
    /// variables, and the line may instead be a JSON `{"command", "args", "env"}` object.
    pub fn parse(command_line: &str) -> Result<Self, agent_client_protocol::Error> {
        let config = AcpAgent::from_str(command_line)?.into_config();
        Ok(Self {
            command: config.command().to_string_lossy().into_owned(),
            args: config.arguments().to_vec(),
            env: config.environment().clone(),
        })
    }

    pub(crate) fn expand(&self, vars: &Vars) -> Result<Self, VarError> {
        Ok(Self {
            command: vars.expand(&self.command)?,
            args: self.args.iter().map(|arg| vars.expand(arg)).collect::<Result<_, _>>()?,
            env: expand_values(&self.env, vars)?,
        })
    }
}

impl Endpoint {
    pub(crate) fn expand(&self, vars: &Vars) -> Result<Self, VarError> {
        Ok(Self { url: vars.expand(&self.url)?, headers: expand_values(&self.headers, vars)? })
    }
}

impl TryFrom<WebSocketAgentFields> for WebSocketAgent {
    type Error = WebSocketAgentError;

    fn try_from(fields: WebSocketAgentFields) -> Result<Self, Self::Error> {
        let WebSocketAgentFields { url, headers, endpoint, launch, terminate } = fields;
        let endpoint = match (url, endpoint, headers) {
            (Some(url), None, headers) => EndpointSource::Fixed(Endpoint { url, headers: headers.unwrap_or_default() }),
            (None, Some(command), None) => EndpointSource::Command(command),
            (None, Some(_), Some(_)) => return Err(WebSocketAgentError::HeadersWithoutUrl),
            (Some(_), Some(_), _) => return Err(WebSocketAgentError::ConflictingEndpoints),
            (None, None, _) => return Err(WebSocketAgentError::MissingEndpoint),
        };
        if terminate.is_some() && launch.is_none() {
            return Err(WebSocketAgentError::TerminateWithoutLaunch);
        }
        Ok(Self { endpoint, launch, terminate })
    }
}

/// An `acp.json` file.
#[derive(Default, Deserialize)]
struct AcpConfig {
    #[serde(default)]
    agents: BTreeMap<String, AgentConfig>,
}

/// A WebSocket agent as written, before checking that its fields go together.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebSocketAgentFields {
    url: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    endpoint: Option<AgentCommand>,
    launch: Option<AgentCommand>,
    terminate: Option<AgentCommand>,
}

fn expand_values(values: &BTreeMap<String, String>, vars: &Vars) -> Result<BTreeMap<String, String>, VarError> {
    values.iter().map(|(name, value)| Ok((name.clone(), vars.expand(value)?))).collect()
}
