use super::error::{ClientError, Result};
use super::sigv4::SigV4HttpClient;
use super::{
    ClientOptions,
    handler::Handler,
    oauth::{self, OAuthSettings},
};
use crate::server::McpServer;
use aether_auth::SigV4Signer;
use reqwest::header::HeaderMap;
use rmcp::{
    ClientLifecycleMode, RoleClient,
    model::ProtocolVersion,
    serve_client_with_lifecycle,
    service::{ClientInitializeError, RunningService},
    transport::{
        IntoTransport, StreamableHttpClientTransport, TokioChildProcess,
        streamable_http_client::StreamableHttpClientTransportConfig,
    },
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    net::UnixStream,
    process::{ChildStderr, Command},
};

#[derive(Debug, Clone)]
pub enum Transport {
    Stdio { command: String, args: Vec<String>, env: HashMap<String, String>, cwd: Option<PathBuf> },
    Http { url: String, headers: HeaderMap, auth: HttpAuth },
    InProcess(McpServer),
    Unix(PathBuf),
}

#[derive(Debug, Clone)]
pub enum HttpAuth {
    None,
    OAuth(OAuthSettings),
    SigV4(SigV4Signer),
}

pub(super) type ClientService = RunningService<RoleClient, Arc<Handler>>;

impl Transport {
    pub(crate) fn supports_oauth(&self, options: &ClientOptions) -> bool {
        options.oauth_prompts().is_some() && matches!(self, Self::Http { auth: HttpAuth::OAuth(_), .. })
    }

    pub(super) async fn connect(self, handler: Handler, options: &ClientOptions) -> Result<ClientService> {
        let supports_oauth = self.supports_oauth(options);
        let handler = Arc::new(handler);
        let connected = match self {
            Self::Stdio { command, args, env, cwd } => connect_stdio(handler, command, args, env, cwd).await,
            Self::Http { url, headers, auth } => connect_http(handler, url, headers, auth, options).await,
            Self::InProcess(server) => serve(handler, server.serve_in_memory()).await,
            Self::Unix(path) => connect_unix(handler, path).await,
        };
        match connected {
            Err(ClientError::Connect { server, source }) if supports_oauth && requires_authorization(&source) => {
                Err(ClientError::AuthRequired { server, challenge: source.auth_challenge().map(str::to_string) })
            }
            connected => connected,
        }
    }

    pub(super) async fn authorize(
        self,
        handler: Handler,
        options: &ClientOptions,
        challenge: Option<String>,
    ) -> Result<ClientService> {
        let server = handler.server().to_string();
        let Self::Http { url, headers, auth: HttpAuth::OAuth(oauth) } = self else {
            return Err(ClientError::OAuthUnavailable { server });
        };
        let auth_client = oauth::authorize(&server, &url, oauth, options, challenge).await?;
        let transport = StreamableHttpClientTransport::with_client(auth_client, http_config(url, headers));
        serve(Arc::new(handler), transport).await
    }
}

fn client_lifecycle_mode() -> ClientLifecycleMode {
    ClientLifecycleMode::Auto {
        preferred_versions: vec![
            ProtocolVersion::V_2026_07_28,
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
            ProtocolVersion::V_2024_11_05,
        ],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    }
}

async fn connect_stdio(
    handler: Arc<Handler>,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<PathBuf>,
) -> Result<ClientService> {
    let mut cmd = Command::new(&command);
    cmd.args(&args).envs(&env);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let (process, stderr) = TokioChildProcess::builder(cmd)
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| ClientError::Spawn { command, source })?;

    if let Some(stderr) = stderr {
        spawn_stderr_logger(handler.server().to_string(), stderr);
    }

    serve(handler, process).await
}

async fn connect_http(
    handler: Arc<Handler>,
    url: String,
    headers: HeaderMap,
    auth: HttpAuth,
    options: &ClientOptions,
) -> Result<ClientService> {
    let config = http_config(url, headers);
    match auth {
        HttpAuth::None => serve(handler, StreamableHttpClientTransport::from_config(config)).await,
        HttpAuth::OAuth(oauth) => match oauth::restore(handler.server(), &config.uri, &oauth, options).await {
            Some(auth_client) => serve(handler, StreamableHttpClientTransport::with_client(auth_client, config)).await,
            None => serve(handler, StreamableHttpClientTransport::from_config(config)).await,
        },
        HttpAuth::SigV4(signer) => {
            serve(handler, StreamableHttpClientTransport::with_client(SigV4HttpClient::new(signer), config)).await
        }
    }
}

async fn connect_unix(handler: Arc<Handler>, path: PathBuf) -> Result<ClientService> {
    let stream = UnixStream::connect(&path).await.map_err(|source| ClientError::SocketConnect { path, source })?;
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
        .map_err(|source| ClientError::connect(server, source))
}

fn requires_authorization(error: &ClientInitializeError) -> bool {
    error.is_authorization_required() || error.auth_challenge().is_some()
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
