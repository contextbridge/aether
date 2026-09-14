use acp_utils::websocket::WebSocketTransport;
use agent_client_protocol::schema::v2::SessionId;
use thiserror::Error;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, Request, StatusCode, Uri};
use wisp::run_remote_tui;
use wisp::settings::load_or_create_settings;

#[derive(clap::Args)]
pub struct ClientArgs {
    /// Remote Aether WebSocket URL (use wss:// for an authenticating TLS proxy).
    #[arg(default_value = "ws://127.0.0.1:8765")]
    pub url: String,

    /// Saved session to resume instead of the server's current session.
    #[arg(long, value_name = "ID")]
    pub session: Option<String>,

    /// HTTP header for a gateway/proxy, in 'Name: value' format. Repeatable.
    #[arg(short = 'H', long = "header", value_name = "HEADER")]
    pub headers: Vec<String>,

    /// Directory for client UI logs (default: /tmp/wisp-logs).
    #[arg(long)]
    pub log_dir: Option<String>,
}

#[derive(Debug, Error)]
pub enum ClientRunError {
    #[error("Invalid remote URL; expected ws:// or wss:// with a host")]
    InvalidUrl,
    #[error("Invalid HTTP header #{index}; expected 'Name: value' with a valid HTTP name and value")]
    InvalidHeader { index: usize },
    #[error("The server already has a client attached; disconnect it before connecting again")]
    ServerOccupied,
    #[error("Failed to establish the remote WebSocket connection; check the address, network, and proxy configuration")]
    Handshake,
    #[error(transparent)]
    Tui(#[from] wisp::error::AppError),
}

impl ClientArgs {
    /// Validate all connection inputs before performing any network I/O.
    pub fn connection_request(&self) -> Result<Request<()>, ClientRunError> {
        let uri: Uri = self.url.parse().map_err(|_| ClientRunError::InvalidUrl)?;
        if !matches!(uri.scheme_str(), Some("ws" | "wss")) || uri.host().is_none_or(str::is_empty) {
            return Err(ClientRunError::InvalidUrl);
        }
        let mut request = uri.into_client_request().map_err(|_| ClientRunError::InvalidUrl)?;
        for (index, header) in self.headers.iter().enumerate() {
            let invalid = || ClientRunError::InvalidHeader { index: index + 1 };
            let (name, value) = header.split_once(':').ok_or_else(invalid)?;
            let name = HeaderName::from_bytes(name.as_bytes()).map_err(|_| invalid())?;
            let mut value = HeaderValue::from_str(value.trim_matches([' ', '\t'])).map_err(|_| invalid())?;
            value.set_sensitive(true);
            request.headers_mut().append(name, value);
        }
        Ok(request)
    }
}

pub async fn run_client(args: ClientArgs) -> Result<(), ClientRunError> {
    let request = args.connection_request()?;
    let (socket, _) = connect_async(request).await.map_err(|error| match error {
        tungstenite::Error::Http(response) if response.status() == StatusCode::CONFLICT => {
            ClientRunError::ServerOccupied
        }
        _ => ClientRunError::Handshake,
    })?;

    let settings = load_or_create_settings();
    run_remote_tui(
        WebSocketTransport::new(socket),
        args.session.map(SessionId::new),
        settings,
        args.log_dir.as_deref(),
    )
    .await?;

    Ok(())
}
