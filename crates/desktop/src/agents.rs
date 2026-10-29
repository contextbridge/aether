use agent_client_protocol::AcpAgent;
use gpui::Context;
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::str::FromStr;
use utils::fs::{JsonFileError, read_json};
use utils::variables::{VarError, Vars};

pub const AGENTS_FILE: &str = "acp.json";

pub const DEFAULT_AGENT: &str = "aether";

pub struct Agents {
    builtin: CommandLine,
    user_file: Option<PathBuf>,
    catalogs: HashMap<PathBuf, Rc<AgentCatalog>>,
}

#[derive(Debug)]
pub struct AgentCatalog {
    agents: Vec<AgentProfile>,
    problems: Vec<JsonFileError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentProfile {
    pub name: String,
    pub workspace: PathBuf,
    pub config: AgentConfig,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AgentConfig {
    #[serde(flatten)]
    pub transport: AgentTransport,
    #[serde(default)]
    pub diff: Option<EndpointSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AgentTransport {
    Stdio(CommandLine),
    WebSocket(EndpointSource),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandLine {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "EndpointFields")]
pub enum EndpointSource {
    Fixed(Endpoint),
    Command(CommandLine),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Endpoint {
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AgentConfigError {
    #[error("a websocket agent or `diff` needs a `url` or an `endpoint` command")]
    MissingEndpoint,
    #[error("a websocket agent or `diff` takes a `url` or an `endpoint` command, not both")]
    ConflictingEndpoints,
    #[error("`headers` go with a `url`, since an `endpoint` command prints its own")]
    HeadersWithoutUrl,
}

pub fn default_agents_file() -> Option<PathBuf> {
    utils::path::aether_home().map(|home| home.join("desktop").join(AGENTS_FILE))
}

pub fn project_agents_file(project: &Path) -> PathBuf {
    project.join(".aether").join(AGENTS_FILE)
}

impl Agents {
    pub fn new(builtin: CommandLine, user_file: Option<PathBuf>) -> Self {
        Self { builtin, user_file, catalogs: HashMap::new() }
    }

    pub fn catalog(&mut self, project: &Path) -> Rc<AgentCatalog> {
        if let Some(catalog) = self.catalogs.get(project) {
            return catalog.clone();
        }
        let catalog = Rc::new(read_catalog(&self.builtin, self.user_file.as_deref(), project));
        self.catalogs.insert(project.to_path_buf(), catalog.clone());
        catalog
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.catalogs.clear();
        cx.notify();
    }
}

impl AgentCatalog {
    pub fn agents(&self) -> &[AgentProfile] {
        &self.agents
    }

    pub fn get(&self, name: &str) -> Option<&AgentProfile> {
        self.agents.iter().find(|agent| agent.name == name)
    }

    pub fn default_agent(&self) -> &AgentProfile {
        self.get(DEFAULT_AGENT).expect("every catalog has the default agent")
    }

    pub fn problems(&self) -> &[JsonFileError] {
        &self.problems
    }
}

impl AgentProfile {
    pub(crate) fn vars(&self) -> Vars {
        Vars::new().with("WORKSPACE", self.workspace.to_string_lossy())
    }
}

impl CommandLine {
    pub fn new(command: impl Into<String>) -> Self {
        Self { command: command.into(), args: Vec::new(), env: BTreeMap::new() }
    }

    #[must_use]
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

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

impl TryFrom<EndpointFields> for EndpointSource {
    type Error = AgentConfigError;

    fn try_from(fields: EndpointFields) -> Result<Self, Self::Error> {
        match (fields.url, fields.endpoint, fields.headers) {
            (Some(url), None, headers) => Ok(Self::Fixed(Endpoint { url, headers: headers.unwrap_or_default() })),
            (None, Some(command), None) => Ok(Self::Command(command)),
            (None, Some(_), Some(_)) => Err(AgentConfigError::HeadersWithoutUrl),
            (Some(_), Some(_), _) => Err(AgentConfigError::ConflictingEndpoints),
            (None, None, _) => Err(AgentConfigError::MissingEndpoint),
        }
    }
}

#[derive(Default, Deserialize)]
struct AcpConfig {
    #[serde(default)]
    agents: BTreeMap<String, AgentConfig>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointFields {
    url: Option<String>,
    headers: Option<BTreeMap<String, String>>,
    endpoint: Option<CommandLine>,
}

fn read_catalog(builtin: &CommandLine, user_file: Option<&Path>, project: &Path) -> AgentCatalog {
    let builtin = AgentConfig { transport: AgentTransport::Stdio(builtin.clone()), diff: None };
    let mut configs = BTreeMap::from([(DEFAULT_AGENT.to_owned(), builtin)]);
    let mut problems = Vec::new();
    for file in user_file.map(Path::to_path_buf).into_iter().chain([project_agents_file(project)]) {
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

fn expand_values(values: &BTreeMap<String, String>, vars: &Vars) -> Result<BTreeMap<String, String>, VarError> {
    values.iter().map(|(name, value)| Ok((name.clone(), vars.expand(value)?))).collect()
}
