use super::{
    ClientOptions,
    handler::Handler,
    oauth::{self, ResolvedOAuth},
};
use crate::config::McpOAuthConfig;
use crate::error::{McpError, Result};
use crate::protocol::{client_lifecycle_mode, serve_in_memory};
use crate::server::McpServer;
use reqwest::header::HeaderMap;
use rmcp::{
    RoleClient, RoleServer, serve_client_with_lifecycle,
    service::{DynService, RunningService},
    transport::{
        IntoTransport, StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
    process::{ChildStderr, Command},
};

#[derive(Debug, Clone)]
pub enum Transport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String> },
    Http { url: String, headers: HeaderMap, oauth: Option<McpOAuthConfig> },
    InProcess(McpServer),
    Unix(PathBuf),
}

pub(super) struct Session {
    pub(super) service: ClientService,
    pub(super) hosted: Option<RunningService<RoleServer, Box<dyn DynService<RoleServer>>>>,
}

pub(super) type ClientService = RunningService<RoleClient, Arc<Handler>>;

impl Transport {
    pub fn supports_oauth(&self, options: &ClientOptions) -> bool {
        options.oauth_prompts().is_some()
            && matches!(self, Self::Http { headers, oauth, .. } if ResolvedOAuth::resolve(headers, oauth.as_ref(), options).is_some())
    }

    pub(super) async fn connect(self, handler: Handler, options: &ClientOptions) -> Result<Session> {
        let handler = Arc::new(handler);
        match self {
            Self::Stdio { command, args, env } => {
                connect_stdio(handler, command, args, env, options.cwd.as_deref()).await.map(Session::from)
            }
            Self::Http { url, headers, oauth } => {
                connect_http(handler, url, headers, oauth, options).await.map(Session::from)
            }
            Self::InProcess(server) => connect_in_process(handler, server).await,
            Self::Unix(path) => connect_unix(handler, path).await.map(Session::from),
        }
    }

    pub(super) async fn authorize(
        self,
        handler: Handler,
        options: &ClientOptions,
        challenge: Option<String>,
    ) -> Result<Session> {
        let server = handler.server().to_string();
        let resolved = match &self {
            Self::Http { headers, oauth, .. } => ResolvedOAuth::resolve(headers, oauth.as_ref(), options),
            Self::Stdio { .. } | Self::InProcess(_) | Self::Unix(_) => None,
        };
        let (Self::Http { url, headers, .. }, Some(resolved)) = (self, resolved) else {
            return Err(McpError::OAuthUnavailable { server });
        };
        let auth_client = oauth::authorize(&server, &url, resolved, options, challenge).await?;
        let transport = StreamableHttpClientTransport::with_client(auth_client, http_config(url, headers));
        serve(Arc::new(handler), transport).await.map(Session::from)
    }
}

impl From<ClientService> for Session {
    fn from(service: ClientService) -> Self {
        Self { service, hosted: None }
    }
}

async fn connect_stdio(
    handler: Arc<Handler>,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<&Path>,
) -> Result<ClientService> {
    let mut cmd = Command::new(&command);
    cmd.args(&args).envs(&env);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let (process, stderr) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| McpError::Spawn { command, source })?;

    if let Some(stderr) = stderr {
        spawn_stderr_logger(handler.server().to_string(), stderr);
    }

    serve(handler, process).await
}

async fn connect_http(
    handler: Arc<Handler>,
    url: String,
    headers: HeaderMap,
    oauth: Option<McpOAuthConfig>,
    options: &ClientOptions,
) -> Result<ClientService> {
    let server = handler.server().to_string();
    let resolved = ResolvedOAuth::resolve(&headers, oauth.as_ref(), options);
    let restored = match (options.oauth_store.as_ref(), &resolved) {
        (Some(store), Some(resolved)) => oauth::restore(&server, &url, resolved, store).await,
        _ => None,
    };

    let config = http_config(url, headers);
    let result = if let Some(auth_client) = restored {
        tracing::debug!(server, "Using stored OAuth credentials");
        let transport = StreamableHttpClientTransport::with_client(auth_client, config);
        serve_client_with_lifecycle(handler, transport, client_lifecycle_mode()).await
    } else {
        let transport = StreamableHttpClientTransport::from_config(config);
        serve_client_with_lifecycle(handler, transport, client_lifecycle_mode()).await
    };

    result.map_err(|error| {
        let challenge = error.auth_challenge().map(str::to_string);
        let authorization_required = error.is_authorization_required() || challenge.is_some();
        if authorization_required && resolved.is_some() && options.oauth_prompts().is_some() {
            McpError::AuthRequired { server, challenge }
        } else {
            McpError::connect(server, error)
        }
    })
}

async fn connect_in_process(handler: Arc<Handler>, server: McpServer) -> Result<Session> {
    let name = handler.server().to_string();
    let (hosted, service) = serve_in_memory(server.service(), handler).await;
    let service = service.map_err(|source| McpError::connect(name, source))?;
    Ok(Session { service, hosted: Some(hosted?) })
}

async fn connect_unix(handler: Arc<Handler>, path: PathBuf) -> Result<ClientService> {
    let stream = UnixStream::connect(&path).await.map_err(|source| McpError::SocketConnect { path, source })?;
    serve(handler, stream).await
}

async fn serve<T, U, V>(handler: Arc<Handler>, transport: T) -> Result<ClientService>
where
    T: IntoTransport<RoleClient, U, V>,
    U: std::error::Error + Send + Sync + 'static,
{
    let server = handler.server().to_string();
    serve_client_with_lifecycle(handler, transport, client_lifecycle_mode())
        .await
        .map_err(|source| McpError::connect(server, source))
}

fn http_config(url: String, headers: HeaderMap) -> StreamableHttpClientTransportConfig {
    let headers = headers.into_iter().filter_map(|(name, value)| Some((name?, value))).collect();
    StreamableHttpClientTransportConfig::with_uri(url).custom_headers(headers)
}

fn spawn_stderr_logger(server: String, stderr: ChildStderr) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => tracing::info!(server = %server, stderr = %line, "MCP server stderr"),
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(server = %server, %error, "failed to read MCP server stderr");
                    break;
                }
            }
        }
    });
}
