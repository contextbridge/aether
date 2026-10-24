//! Where a thread's agent runs, and how the app connects to it.

use crate::agents::{AgentCommand, AgentProfile, AgentTransport, Endpoint, EndpointSource};
use acp_utils::websocket::WebSocketTransport;
use agent_client_protocol::schema::v2::{Implementation, SessionId};
use agent_client_protocol::{self as acp, Agent, ByteStreams, Client, ConnectTo};
use client_core::error::ClientError;
use client_core::session::{Connection, Session};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::Instant;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Request, StatusCode};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use utils::variables::{VarError, Vars};

/// Where a thread's agent runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTarget {
    /// The directory the thread works in, which a stdio agent is started in.
    pub cwd: PathBuf,
    pub agent: AgentProfile,
    /// What the agent's `launch` command started, once it has run.
    pub instance: Option<InstanceId>,
}

/// Names what an agent's `launch` command started, such as a microVM, to its other commands.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct InstanceId(String);

/// Whether a connection opens a fresh session or resumes a saved one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectMode {
    NewSession,
    /// Resumes the saved session, replaying its history from the start.
    Resume(SessionId),
}

/// How long a launched instance's server may take to answer at its endpoint, such as while a
/// microVM installs dependencies before its agent listens.
pub const STARTUP_WINDOW: Duration = Duration::from_mins(5);

pub type ConnectorFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ConnectError>> + Send + 'a>>;

/// Starts, connects to and ends the agent of a [`SessionTarget`].
pub trait Connector: Send + Sync + 'static {
    /// Starts the instance the target's agent runs on, if its agent has a `launch` command.
    fn launch<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, Option<InstanceId>>;

    /// Connects to the target's agent, with its session open.
    fn connect<'a>(&'a self, target: &'a SessionTarget, mode: ConnectMode) -> ConnectorFuture<'a, Session>;

    fn diff_endpoint<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, Option<Endpoint>>;

    /// Ends the target's instance, if it has one and its agent has a `terminate` command.
    fn terminate<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, ()>;
}

/// Reaches each thread's agent as its [`AgentConfig`](crate::agents::AgentConfig) says.
///
/// A stdio agent runs as a child process of the app in the thread's directory, and is killed
/// when its connection is dropped. A WebSocket agent is connected to at its endpoint, which for an
/// agent with a `launch` command is tried again for up to [`STARTUP_WINDOW`] while the server
/// behind it starts. The agent's commands run in its workspace, and those that answer print JSON
/// on stdout.
pub struct AgentConnector;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error(transparent)]
    Variable(#[from] VarError),
    #[error("failed to start `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: io::Error,
    },
    #[error("`{program}` failed ({status}): {stderr}")]
    CommandFailed {
        program: String,
        status: ExitStatus,
        /// The last line it printed on stderr.
        stderr: String,
    },
    #[error("`{program}` printed unexpected output: {source}")]
    CommandOutput {
        program: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid WebSocket URL `{url}`")]
    InvalidUrl { url: String },
    #[error("invalid WebSocket header `{name}`")]
    InvalidHeader { name: String },
    #[error("another client is already attached to the agent")]
    Occupied,
    #[error("failed to connect to the agent's WebSocket: {0}")]
    WebSocket(#[source] Box<tungstenite::Error>),
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl SessionTarget {
    /// Runs `agent` for a thread working in `cwd`, on a new instance if the agent launches one.
    pub fn new(cwd: PathBuf, agent: AgentProfile) -> Self {
        Self { cwd, agent, instance: None }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// What `${VAR}` references in the agent's configuration expand to besides the environment.
    pub(crate) fn vars(&self) -> Vars {
        let mut vars = Vars::new().with("WORKSPACE", self.agent.workspace.to_string_lossy());
        if let Some(instance) = &self.instance {
            vars.insert("INSTANCE_ID", instance.as_str());
        }
        vars
    }
}

impl InstanceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ConnectMode {
    /// Opens this mode's session on a freshly initialized `connection`. An Aether remote server
    /// works in its own directory, and a new session there is the one it already has live.
    pub(crate) async fn open(self, connection: Connection) -> Result<Session, ClientError> {
        match (self, connection.is_remote_server()) {
            (Self::NewSession, true) => connection.open_remote(None).await,
            (Self::Resume(session_id), true) => connection.open_remote(Some(session_id)).await,
            (Self::NewSession, false) => connection.new_session().await,
            (Self::Resume(session_id), false) => connection.resume_session(session_id).await,
        }
    }
}

impl Connector for AgentConnector {
    fn launch<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, Option<InstanceId>> {
        Box::pin(async move {
            let Some(launch) = target.agent.config.launch() else {
                return Ok(None);
            };
            let launched: Launched = run_for_json(launch, target).await?;
            Ok(Some(InstanceId(launched.id)))
        })
    }

    fn connect<'a>(&'a self, target: &'a SessionTarget, mode: ConnectMode) -> ConnectorFuture<'a, Session> {
        Box::pin(async move {
            let cwd = target.cwd.clone();
            let connection = match &target.agent.config.transport {
                AgentTransport::Stdio(command) => {
                    let process = AgentProcess::spawn(&command.expand(&target.vars())?, &target.cwd)?;
                    Connection::connect_to(client_info(), process, cwd).await?
                }
                AgentTransport::WebSocket(agent) => {
                    let endpoint = resolve(&agent.endpoint, target).await?;
                    let startup = if agent.launch.is_some() { STARTUP_WINDOW } else { Duration::ZERO };
                    Connection::connect_to(client_info(), open_websocket(&endpoint, startup).await?, cwd).await?
                }
            };
            Ok(mode.open(connection).await?)
        })
    }

    fn diff_endpoint<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, Option<Endpoint>> {
        Box::pin(async move {
            match &target.agent.config.diff {
                Some(source) => Ok(Some(resolve(source, target).await?)),
                None => Ok(None),
            }
        })
    }

    fn terminate<'a>(&'a self, target: &'a SessionTarget) -> ConnectorFuture<'a, ()> {
        Box::pin(async move {
            if let Some(terminate) = target.agent.config.terminate().filter(|_| target.instance.is_some()) {
                run(&terminate.expand(&target.vars())?, &target.agent.workspace).await?;
            }
            Ok(())
        })
    }
}

/// How the app introduces itself to every agent it connects to.
pub(crate) fn client_info() -> Implementation {
    Implementation::new("aether-desktop", env!("CARGO_PKG_VERSION"))
}

const FIRST_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

/// What a `launch` command prints.
#[derive(Deserialize)]
struct Launched {
    id: String,
}

/// A started agent process, served as an ACP transport over its stdio.
struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl AgentProcess {
    fn spawn(command: &AgentCommand, cwd: &Path) -> Result<Self, ConnectError> {
        let mut child = Command::new(&command.command)
            .args(&command.args)
            .envs(&command.env)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ConnectError::Spawn { program: command.command.clone(), source })?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            unreachable!("both pipes were requested at spawn");
        };
        Ok(Self { child, stdin, stdout })
    }
}

impl ConnectTo<Client> for AgentProcess {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> Result<(), acp::Error> {
        // Held for as long as the connection runs: dropping it, when the connection ends or is
        // aborted, kills the process.
        let Self { child: _child, stdin, stdout } = self;
        ConnectTo::<Client>::connect_to(ByteStreams::new(stdin.compat_write(), stdout.compat()), client).await
    }
}

async fn resolve(source: &EndpointSource, target: &SessionTarget) -> Result<Endpoint, ConnectError> {
    match source {
        EndpointSource::Fixed(endpoint) => Ok(endpoint.expand(&target.vars())?),
        EndpointSource::Command(command) => run_for_json(command, target).await,
    }
}

/// Runs `command`, expanded for `target`, and reads what it prints on stdout as JSON.
async fn run_for_json<T: DeserializeOwned>(command: &AgentCommand, target: &SessionTarget) -> Result<T, ConnectError> {
    let command = command.expand(&target.vars())?;
    let stdout = run(&command, &target.agent.workspace).await?;
    serde_json::from_slice(&stdout).map_err(|source| ConnectError::CommandOutput { program: command.command, source })
}

/// Runs `command` in `dir` to completion, returning what it printed on stdout if it succeeded.
///
/// Dropping the future kills the command, so that stopping a thread while its agent launches
/// stops the launch too.
async fn run(command: &AgentCommand, dir: &Path) -> Result<Vec<u8>, ConnectError> {
    let output = Command::new(&command.command)
        .args(&command.args)
        .envs(&command.env)
        .current_dir(dir)
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|source| ConnectError::Spawn { program: command.command.clone(), source })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last_line = stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default();
        return Err(ConnectError::CommandFailed {
            program: command.command.clone(),
            status: output.status,
            stderr: last_line.to_owned(),
        });
    }
    Ok(output.stdout)
}

/// Opens a WebSocket to `endpoint`, served as an ACP transport. While nothing answers there yet,
/// it tries again, less often each time, until `startup` has passed.
async fn open_websocket(
    endpoint: &Endpoint,
    startup: Duration,
) -> Result<impl ConnectTo<Client> + 'static, ConnectError> {
    let deadline = Instant::now() + startup;
    let mut delay = FIRST_RETRY_DELAY;
    loop {
        match connect_async(handshake_request(endpoint)?).await {
            Ok((socket, _)) => return Ok(WebSocketTransport::new(socket)),
            Err(error) if is_starting(&error) && Instant::now() + delay < deadline => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(MAX_RETRY_DELAY);
            }
            Err(tungstenite::Error::Http(response)) if response.status() == StatusCode::CONFLICT => {
                return Err(ConnectError::Occupied);
            }
            Err(error) => return Err(ConnectError::WebSocket(Box::new(error))),
        }
    }
}

fn handshake_request(endpoint: &Endpoint) -> Result<Request<()>, ConnectError> {
    let mut request = endpoint
        .url
        .as_str()
        .into_client_request()
        .map_err(|_| ConnectError::InvalidUrl { url: endpoint.url.clone() })?;
    for (name, value) in &endpoint.headers {
        let invalid = || ConnectError::InvalidHeader { name: name.clone() };
        let header = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
        let mut value = HeaderValue::from_str(value).map_err(|_| invalid())?;
        // Headers usually carry credentials, which this keeps out of debug output.
        value.set_sensitive(true);
        request.headers_mut().append(header, value);
    }
    Ok(request)
}

/// Whether `error` says nothing answers at the endpoint yet: the connection is refused, or the
/// proxy in front of the server cannot reach it.
fn is_starting(error: &tungstenite::Error) -> bool {
    match error {
        tungstenite::Error::Io(_) => true,
        tungstenite::Error::Http(response) => response.status().is_server_error(),
        _ => false,
    }
}
