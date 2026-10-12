use super::FakeMcpServer;
use crate::client::{Elicitation, ElicitationRequest, OAuthSettings, Transport};
use crate::gateway::ServerSpec;
use aether_auth::OAuthClientRegistration;
use axum::extract::{Request, State};
use axum::http::{StatusCode, Uri, header::WWW_AUTHENTICATE};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub struct FakeAuthServer {
    url: String,
    auth: Arc<AuthState>,
    task: JoinHandle<()>,
}

pub struct OAuthPrompt {
    pub elicitation: Box<ElicitationRequest>,
    pub url: Url,
}

pub const CLIENT_METADATA_URL: &str = "https://client.example/oauth/client-metadata.json";

impl FakeAuthServer {
    pub async fn start(server: FakeMcpServer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind the fake authorization server");
        let origin = format!("http://{}", listener.local_addr().expect("a bound address"));
        let auth = Arc::new(AuthState { origin: origin.clone(), bearers: Mutex::new(Vec::new()) });
        let mcp = StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig::default(),
        );
        let protected =
            Router::new().nest_service("/mcp", mcp).layer(from_fn_with_state(Arc::clone(&auth), require_token));
        let app = Router::new()
            .route("/token", post(token))
            .fallback(metadata)
            .with_state(Arc::clone(&auth))
            .merge(protected);
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve the fake authorization server");
        });
        Self { url: format!("{origin}/mcp"), auth, task }
    }

    pub fn transport(&self) -> Transport {
        self.transport_with(Some(client_metadata()))
    }

    pub fn transport_with(&self, oauth: Option<OAuthSettings>) -> Transport {
        Transport::Http { url: self.url.clone(), headers: HeaderMap::new(), oauth }
    }

    pub fn spec(&self, name: &str) -> ServerSpec {
        ServerSpec { name: name.to_string(), transport: self.transport() }
    }

    pub fn received_bearer(&self, token: &str) -> bool {
        self.auth.bearers.lock().expect("bearers lock").iter().any(|bearer| bearer == token)
    }
}

impl Drop for FakeAuthServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl OAuthPrompt {
    pub async fn receive(host: &mut mpsc::Receiver<Elicitation>) -> Self {
        let Some(Elicitation::Request(elicitation)) = host.recv().await else {
            panic!("expected the authorization prompt")
        };
        let ElicitRequestParams::UrlElicitationParams { url, elicitation_id, .. } = &elicitation.request else {
            panic!("expected a URL elicitation")
        };
        assert_eq!(elicitation_id, "oauth");
        let url = Url::parse(url).expect("an authorization URL");
        Self { elicitation, url }
    }

    pub fn query(&self, name: &str) -> String {
        self.url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned()).unwrap_or_default()
    }

    pub fn callback_url(&self) -> String {
        callback_url(self.url.as_str())
    }

    pub async fn approve(self) {
        let url = self.url.to_string();
        self.elicitation.respond(ElicitResult::new(ElicitationAction::Accept));
        complete_authorization(&url).await;
    }
}

pub async fn complete_authorization(authorization_url: &str) {
    reqwest::get(callback_url(authorization_url)).await.expect("the loopback callback responds");
}

fn callback_url(authorization_url: &str) -> String {
    let url = Url::parse(authorization_url).expect("an authorization URL");
    let query = |name: &str| url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned());
    let mut callback = Url::parse(&query("redirect_uri").expect("a redirect URI")).expect("a valid redirect URI");
    callback
        .query_pairs_mut()
        .append_pair("code", "test-code")
        .append_pair("state", &query("state").unwrap_or_default());
    callback.to_string()
}

pub fn client_metadata() -> OAuthSettings {
    OAuthSettings {
        registration: OAuthClientRegistration::ClientMetadata(CLIENT_METADATA_URL.to_string()),
        callback_port: 0,
    }
}

pub const ACCESS_TOKEN: &str = "access-token";

struct AuthState {
    origin: String,
    bearers: Mutex<Vec<String>>,
}

async fn require_token(State(auth): State<Arc<AuthState>>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string);
    if let Some(bearer) = bearer {
        auth.bearers.lock().expect("bearers lock").push(bearer.clone());
        if bearer == ACCESS_TOKEN {
            return next.run(request).await;
        }
    }
    let challenge = format!("Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"", auth.origin);
    (StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, challenge)]).into_response()
}

async fn token() -> Json<Value> {
    Json(json!({ "access_token": ACCESS_TOKEN, "token_type": "Bearer", "expires_in": 3600 }))
}

async fn metadata(State(auth): State<Arc<AuthState>>, uri: Uri) -> Json<Value> {
    let origin = &auth.origin;
    if uri.path().contains("oauth-protected-resource") {
        return Json(json!({ "resource": format!("{origin}/mcp"), "authorization_servers": [origin] }));
    }
    Json(json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/authorize"),
        "token_endpoint": format!("{origin}/token"),
        "response_types_supported": ["code"],
        "code_challenge_methods_supported": ["S256"],
        "client_id_metadata_document_supported": true
    }))
}
