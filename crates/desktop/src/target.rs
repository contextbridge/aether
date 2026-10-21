//! Where a thread's agent runs, and how the app connects to it.

use agent_client_protocol::schema::v2::{Implementation, SessionId};
use agent_client_protocol::{self as acp, AcpAgent, Agent, ByteStreams, Client, ConnectTo};
use client_core::error::ClientError;
use client_core::session::{Connection, Session};
use std::fmt;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::str::FromStr;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};

/// Where a thread's agent runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionTarget {
    /// An agent process on this machine, started in `cwd`.
    Local { cwd: PathBuf, agent: AgentCommandLine },
}

/// The command line that starts an agent process speaking ACP over stdin and stdout, such as
/// `aether acp`.
///
/// Leading `NAME=value` words set environment variables, and the line may instead be a JSON
/// `{"command", "args", "env"}` object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandLine(String);

/// Whether a connection opens a fresh session or resumes a saved one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectMode {
    NewSession,
    /// Resumes the saved session, replaying its history from the start.
    Resume(SessionId),
}

/// The future [`Connector::connect`] returns.
pub type ConnectFuture<'a> = Pin<Box<dyn Future<Output = Result<Session, ConnectError>> + Send + 'a>>;

/// Connects to the agent of a [`SessionTarget`], with its session open.
pub trait Connector: Send + Sync + 'static {
    fn connect<'a>(&'a self, target: &'a SessionTarget, mode: ConnectMode) -> ConnectFuture<'a>;
}

/// Starts each thread's agent as a child process of the app, talking ACP over its stdio.
///
/// The process runs in the thread's directory, and is killed when its connection is dropped.
pub struct LocalConnector;

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("invalid agent command `{command}`: {source}")]
    InvalidAgentCommand {
        command: AgentCommandLine,
        #[source]
        source: Box<acp::Error>,
    },
    #[error("failed to start agent `{command}`: {source}")]
    Spawn {
        command: AgentCommandLine,
        #[source]
        source: io::Error,
    },
    #[error(transparent)]
    Client(#[from] ClientError),
}

impl SessionTarget {
    /// The directory the agent works in.
    pub fn cwd(&self) -> &Path {
        match self {
            Self::Local { cwd, .. } => cwd,
        }
    }
}

impl AgentCommandLine {
    pub fn new(command_line: impl Into<String>) -> Self {
        Self(command_line.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentCommandLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl ConnectMode {
    /// Opens this mode's session on a freshly initialized `connection`.
    pub(crate) async fn open(self, connection: Connection) -> Result<Session, ClientError> {
        match self {
            Self::NewSession => connection.new_session().await,
            Self::Resume(session_id) => connection.resume_session(session_id).await,
        }
    }
}

impl Connector for LocalConnector {
    fn connect<'a>(&'a self, target: &'a SessionTarget, mode: ConnectMode) -> ConnectFuture<'a> {
        Box::pin(async move {
            let SessionTarget::Local { cwd, agent } = target;
            let process = AgentProcess::spawn(agent, cwd)?;
            let connection = Connection::connect_to(client_info(), process, cwd.clone()).await?;
            Ok(mode.open(connection).await?)
        })
    }
}

/// How the app introduces itself to every agent it connects to.
pub(crate) fn client_info() -> Implementation {
    Implementation::new("aether-desktop", env!("CARGO_PKG_VERSION"))
}

/// A started agent process, served as an ACP transport over its stdio.
struct AgentProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl AgentProcess {
    fn spawn(agent: &AgentCommandLine, cwd: &Path) -> Result<Self, ConnectError> {
        // Parsed the way `AcpAgent` parses it, which cannot set a working directory itself.
        let config = AcpAgent::from_str(agent.as_str())
            .map_err(|source| ConnectError::InvalidAgentCommand { command: agent.clone(), source: Box::new(source) })?
            .into_config();
        let mut child = Command::new(config.command())
            .args(config.arguments())
            .envs(config.environment())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|source| ConnectError::Spawn { command: agent.clone(), source })?;
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
