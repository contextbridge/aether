use crate::{RemoteError, RemoteToolRuntime};
use axum::{Router, routing::get};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::never::NeverSessionManager,
};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct HttpOptions {
    pub allowed_hosts: Vec<String>,
    pub max_request_body_bytes: usize,
    pub external_auth: bool,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into(), "::1".into()],
            max_request_body_bytes: 4 * 1024 * 1024,
            external_auth: false,
        }
    }
}

pub async fn serve(
    listener: TcpListener,
    mut runtime: RemoteToolRuntime,
    options: HttpOptions,
    cancellation: CancellationToken,
) -> Result<(), RemoteError> {
    if !listener.local_addr()?.ip().is_loopback() && !options.external_auth {
        return Err(RemoteError::Invalid(
            "non-loopback binding requires --external-auth and authenticating TLS ingress".into(),
        ));
    }
    if options.allowed_hosts.is_empty() || options.max_request_body_bytes == 0 {
        return Err(RemoteError::Invalid("allowed-host and request-body limit must not be empty".into()));
    }
    let mut config = StreamableHttpServerConfig::default().enforce_origin_validation();
    config.legacy_session_mode = false;
    config.json_response = true;
    config.stateless_protocol_metadata_required = true;
    config.cancellation_token = cancellation.clone();
    config.allowed_hosts = options.allowed_hosts;
    config.max_request_body_bytes = options.max_request_body_bytes;
    let handler = runtime.clone();
    let service =
        StreamableHttpService::new(move || Ok(handler.clone()), Arc::new(NeverSessionManager::default()), config);
    let router = Router::new().nest_service("/mcp", service).route("/healthz", get(|| async { "ok" }));
    let shutdown_runtime = runtime.clone();
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            cancellation.cancelled().await;
            shutdown_runtime.cancel_execution();
        })
        .await;
    runtime.shutdown().await?;
    result.map_err(RemoteError::Io)
}
