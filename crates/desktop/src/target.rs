use crate::agents::{AgentProfile, AgentTransport, CommandLine, Endpoint, EndpointSource};
use acp_utils::websocket;
use agent_client_protocol::schema::v2::{Implementation, SessionId};
use agent_client_protocol::{self as acp, Agent, ByteStreams, Client, ConnectTo};
use clankerdiff_client::ConnectionHeader;
use clankerdiff_remote::{AcpTarget, Channel, Invocation, RemoteError, RemoteSpec, Remotes};
use client_core::command::ReviewSource;
use client_core::connection::{Connection, ConnectionError, Session};
use client_core::session_model::WorkspaceAccess;
use serde::de::DeserializeOwned;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{ExitStatus, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::task::{JoinError, spawn_blocking};
use tokio::time::Instant;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use utils::variables::VarError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    Agent { agent: AgentProfile, cwd: PathBuf },
    Remote { remote: RemoteSpec, instance: Option<String> },
}

pub const STARTUP_WINDOW: Duration = Duration::from_mins(5);

pub struct AgentConnector {
    remotes: Arc<dyn Remotes>,
    transport: Arc<dyn Transport>,
}

pub type TransportFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ConnectError>> + Send + 'a>>;

pub trait Transport: Send + Sync + 'static {
    fn stdio<'a>(&'a self, line: &'a CommandLine, dir: &'a Path) -> TransportFuture<'a, Connection>;

    fn websocket<'a>(&'a self, endpoint: &'a Endpoint, startup: Duration) -> TransportFuture<'a, Connection>;

    fn run<'a>(&'a self, line: &'a CommandLine, dir: &'a Path) -> TransportFuture<'a, Vec<u8>>;
}

pub struct ProcessTransport;

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
    CommandFailed { program: String, status: ExitStatus, stderr: String },
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
    #[error(transparent)]
    WebSocket(#[from] websocket::ConnectError),
    #[error(transparent)]
    Client(#[from] ConnectionError),
    #[error(transparent)]
    Remote(#[from] RemoteError),
    #[error("{remote} serves no agent")]
    NoAgent { remote: String },
    #[error("{remote}'s agent needs an `acp.cwd` to work in, since it is not an Aether server that picks its own")]
    NoCwd { remote: String },
    #[error("the thread's instance has not been launched")]
    NotLaunched,
    #[error("the remote's command was interrupted: {0}")]
    Interrupted(#[from] JoinError),
}

impl SessionTarget {
    pub fn agent(cwd: PathBuf, agent: AgentProfile) -> Self {
        Self::Agent { agent, cwd }
    }

    pub fn remote(remote: &RemoteSpec, instance: Option<String>) -> Self {
        let instance = instance.or_else(|| remote.is_static().then(|| remote.name.clone()));
        Self::Remote { remote: remote.clone(), instance }
    }

    pub fn set_instance(&mut self, launched: String) {
        if let Self::Remote { instance, .. } = self {
            *instance = Some(launched);
        }
    }

    pub fn cwd(&self) -> Option<&Path> {
        match self {
            Self::Agent { cwd, .. } => Some(cwd),
            Self::Remote { .. } => None,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::Agent { agent, .. } => &agent.name,
            Self::Remote { remote, .. } => &remote.name,
        }
    }

    pub fn remote_name(&self) -> Option<&str> {
        match self {
            Self::Agent { .. } => None,
            Self::Remote { remote, .. } => Some(&remote.name),
        }
    }

    pub fn instance(&self) -> Option<&str> {
        match self {
            Self::Agent { .. } => None,
            Self::Remote { instance, .. } => instance.as_deref(),
        }
    }
}

impl AgentConnector {
    pub fn new(remotes: Arc<dyn Remotes>, transport: Arc<dyn Transport>) -> Self {
        Self { remotes, transport }
    }

    pub async fn launch(&self, target: &SessionTarget) -> Result<Option<String>, ConnectError> {
        match target {
            SessionTarget::Remote { remote, instance: None } => {
                let (remotes, remote) = (self.remotes.clone(), remote.clone());
                Ok(Some(spawn_blocking(move || remotes.launch(&remote)).await??))
            }
            SessionTarget::Remote { instance: Some(_), .. } | SessionTarget::Agent { .. } => Ok(None),
        }
    }

    pub async fn connect(
        &self,
        target: &SessionTarget,
        session_id: Option<SessionId>,
    ) -> Result<Session, ConnectError> {
        match target {
            SessionTarget::Agent { agent, cwd } => {
                let connection = match &agent.config.transport {
                    AgentTransport::Stdio(command) => {
                        self.transport.stdio(&command.expand(&agent.vars())?, cwd).await?
                    }
                    AgentTransport::WebSocket(source) => {
                        self.transport.websocket(&self.resolve(source, agent).await?, Duration::ZERO).await?
                    }
                };
                Ok(connection.open(session_id, cwd.clone()).await?)
            }
            SessionTarget::Remote { remote, instance } => {
                let instance = instance.as_deref().ok_or(ConnectError::NotLaunched)?;
                let AcpTarget { transport, cwd } =
                    remote.acp(Some(instance))?.ok_or_else(|| ConnectError::NoAgent { remote: remote.name.clone() })?;
                let connection = match transport {
                    Channel::Stdio(invocation) => {
                        self.transport.stdio(&command_line(&invocation), &invocation.dir).await?
                    }
                    Channel::Websocket(invocation) => {
                        let startup = if remote.is_static() { Duration::ZERO } else { STARTUP_WINDOW };
                        self.transport.websocket(&self.endpoint(&invocation).await?, startup).await?
                    }
                };
                match cwd {
                    Some(cwd) => {
                        Ok(connection.open_with_access(session_id, cwd.into(), WorkspaceAccess::Remote).await?)
                    }
                    None => connection.open_remote(session_id).await.map_err(|error| match error {
                        ConnectionError::MissingRemoteContract => ConnectError::NoCwd { remote: remote.name.clone() },
                        error @ ConnectionError::Acp(_) => error.into(),
                    }),
                }
            }
        }
    }

    pub async fn review_source(&self, target: &SessionTarget) -> Result<Option<ReviewSource>, ConnectError> {
        match target {
            SessionTarget::Agent { agent, .. } => match &agent.config.diff {
                Some(source) => Ok(Some(self.resolve(source, agent).await?.review_source()?)),
                None => Ok(None),
            },
            SessionTarget::Remote { remote, instance } => match remote.diff(instance.as_deref())? {
                Some(Channel::Stdio(invocation)) => Ok(Some(ReviewSource::Stdio(invocation))),
                Some(Channel::Websocket(invocation)) => Ok(Some(self.endpoint(&invocation).await?.review_source()?)),
                None => Ok(None),
            },
        }
    }

    async fn resolve(&self, source: &EndpointSource, agent: &AgentProfile) -> Result<Endpoint, ConnectError> {
        match source {
            EndpointSource::Fixed(endpoint) => Ok(endpoint.expand(&agent.vars())?),
            EndpointSource::Command(line) => self.run_for_json(&line.expand(&agent.vars())?, &agent.workspace).await,
        }
    }

    async fn endpoint(&self, invocation: &Invocation) -> Result<Endpoint, ConnectError> {
        self.run_for_json(&command_line(invocation), &invocation.dir).await
    }

    async fn run_for_json<T: DeserializeOwned>(&self, line: &CommandLine, dir: &Path) -> Result<T, ConnectError> {
        let output = self.transport.run(line, dir).await?;
        serde_json::from_slice(&output)
            .map_err(|source| ConnectError::CommandOutput { program: line.command.clone(), source })
    }
}

impl Transport for ProcessTransport {
    fn stdio<'a>(&'a self, line: &'a CommandLine, dir: &'a Path) -> TransportFuture<'a, Connection> {
        Box::pin(async move {
            let mut child = command(line, dir)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .map_err(|source| ConnectError::Spawn { program: line.command.clone(), source })?;
            let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
                unreachable!("both pipes were requested at spawn");
            };
            Ok(Connection::connect_to(client_info(), AgentProcess { child, stdin, stdout }).await?)
        })
    }

    fn websocket<'a>(&'a self, endpoint: &'a Endpoint, startup: Duration) -> TransportFuture<'a, Connection> {
        Box::pin(async move {
            let deadline = Instant::now() + startup;
            let mut delay = FIRST_RETRY_DELAY;
            loop {
                match websocket::connect(handshake_request(endpoint)?).await {
                    Ok(transport) => return Ok(Connection::connect_to(client_info(), transport).await?),
                    Err(websocket::ConnectError::Handshake(error))
                        if is_starting(&error) && Instant::now() + delay < deadline =>
                    {
                        tokio::time::sleep(delay).await;
                        delay = (delay * 2).min(MAX_RETRY_DELAY);
                    }
                    Err(error) => return Err(error.into()),
                }
            }
        })
    }

    fn run<'a>(&'a self, line: &'a CommandLine, dir: &'a Path) -> TransportFuture<'a, Vec<u8>> {
        Box::pin(async move {
            let output = command(line, dir)
                .stdin(Stdio::null())
                .output()
                .await
                .map_err(|source| ConnectError::Spawn { program: line.command.clone(), source })?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                let last_line = stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default();
                return Err(ConnectError::CommandFailed {
                    program: line.command.clone(),
                    status: output.status,
                    stderr: last_line.to_owned(),
                });
            }
            Ok(output.stdout)
        })
    }
}

pub(crate) fn client_info() -> Implementation {
    Implementation::new("aether-desktop", env!("CARGO_PKG_VERSION"))
}

const FIRST_RETRY_DELAY: Duration = Duration::from_secs(1);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);

struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl Endpoint {
    fn review_source(self) -> Result<ReviewSource, ConnectError> {
        let headers = self.connection_headers()?;
        Ok(ReviewSource::WebSocket { url: self.url, headers })
    }

    fn connection_headers(&self) -> Result<Vec<ConnectionHeader>, ConnectError> {
        self.headers
            .iter()
            .map(|(name, value)| {
                format!("{name}: {value}").parse().map_err(|_| ConnectError::InvalidHeader { name: name.clone() })
            })
            .collect()
    }
}

impl ConnectTo<Client> for AgentProcess {
    async fn connect_to(self, client: impl ConnectTo<Agent>) -> Result<(), acp::Error> {
        let Self { child: _child, stdin, stdout } = self;
        ConnectTo::<Client>::connect_to(ByteStreams::new(stdin.compat_write(), stdout.compat()), client).await
    }
}

fn command_line(invocation: &Invocation) -> CommandLine {
    CommandLine { command: invocation.program.clone(), args: invocation.args.clone(), env: invocation.env.clone() }
}

fn command(line: &CommandLine, dir: &Path) -> Command {
    let mut command = Command::new(&line.command);
    command.args(&line.args).envs(&line.env).current_dir(dir).kill_on_drop(true);
    command
}

fn handshake_request(endpoint: &Endpoint) -> Result<Request<()>, ConnectError> {
    let mut request = endpoint
        .url
        .as_str()
        .into_client_request()
        .map_err(|_| ConnectError::InvalidUrl { url: endpoint.url.clone() })?;
    for header in endpoint.connection_headers()? {
        request.headers_mut().append(header.name().clone(), header.value().clone());
    }
    Ok(request)
}

fn is_starting(error: &tungstenite::Error) -> bool {
    match error {
        tungstenite::Error::Io(_) => true,
        tungstenite::Error::Http(response) => response.status().is_server_error(),
        _ => false,
    }
}
