use aether_tool_server::{HttpOptions, RemoteConfig, RemoteError, RemoteToolRuntime, serve};
use clap::{Parser, Subcommand};
use mcp_utils::tool_gateway::command::{McpArgs, run};
use std::{net::SocketAddr, path::PathBuf, process::ExitCode};
use tokio::signal::unix::{SignalKind, signal};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "aether-tool-server", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve remote workspace tools without starting an agent.
    Serve {
        #[arg(long)]
        root_dir: PathBuf,
        #[arg(long, required = true)]
        mcp_config: Vec<PathBuf>,
        #[arg(long, default_value = "127.0.0.1:8080")]
        listen: SocketAddr,
        #[arg(long)]
        external_auth: bool,
        #[arg(long)]
        allowed_host: Vec<String>,
        #[arg(long, default_value_t = 4 * 1024 * 1024)]
        max_request_body_bytes: usize,
    },
    /// Discover and invoke deferred tools from a remote Bash subprocess.
    Mcp(McpArgs),
}

#[tokio::main]
async fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Mcp(args) => match run(args, "aether-tool-server mcp").await {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::from(error.exit_code())
            }
        },
        Command::Serve { root_dir, mcp_config, listen, external_auth, allowed_host, max_request_body_bytes } => {
            let options = HttpOptions {
                allowed_hosts: if allowed_host.is_empty() {
                    HttpOptions::default().allowed_hosts
                } else {
                    allowed_host
                },
                external_auth,
                max_request_body_bytes,
            };
            match start(root_dir, mcp_config, listen, options).await {
                Ok(()) => ExitCode::SUCCESS,
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

async fn start(
    root: PathBuf,
    configs: Vec<PathBuf>,
    listen: SocketAddr,
    options: HttpOptions,
) -> Result<(), RemoteError> {
    if !listen.ip().is_loopback() && !options.external_auth {
        return Err(RemoteError::Invalid("non-loopback binding requires --external-auth".into()));
    }
    let config = RemoteConfig::load(&root, &configs)?;
    let mut runtime = RemoteToolRuntime::new(config).await?;
    let cancellation = CancellationToken::new();
    let mut terminate = signal(SignalKind::terminate())?;
    let shutdown = cancellation.clone();
    let signal = tokio::spawn(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        shutdown.cancel();
    });
    let result = serve(tokio::net::TcpListener::bind(listen).await?, runtime.clone(), options, cancellation).await;
    signal.abort();
    runtime.shutdown().await?;
    result
}
