use aether_auth::OAuthError;
use axum::extract::{Request, State};
use axum::http::{StatusCode, Uri, header::WWW_AUTHENTICATE};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use mcp_utils::McpError;
use mcp_utils::client::{ClientOptions, Elicitation, ElicitationRequest, McpClient, Transport};
use mcp_utils::config::{McpOAuthConfig, RemoteServerConfig};
use mcp_utils::gateway::{McpGateway, ServerSpec, ToolExposure, ToolFilter};
use mcp_utils::model::{
    ElicitRequestParams, ElicitResult, ElicitationAction, ElicitationCapability, FormElicitationCapability,
};
use mcp_utils::testing::FakeMcpServer;
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use serde_json::{Value, json};
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, yield_now};
use utils::mcp_status::McpServerStatus;
use utils::variables::Vars;

#[tokio::test]
async fn connect_reports_the_authorization_challenge() {
    let server = FakeAuthServer::start().await;
    let (events, _host) = mpsc::channel(1);

    let error = McpClient::connect("linear", server.transport(None), &ClientOptions::default().elicitation(events))
        .await
        .err()
        .expect("unauthorized connect fails");

    assert!(matches!(
        error,
        McpError::AuthRequired { server, challenge: Some(challenge) }
            if server == "linear" && challenge.contains("resource_metadata")
    ));
}

#[tokio::test]
async fn an_explicit_authorization_header_disables_oauth() {
    let server = FakeAuthServer::start().await;
    let (events, _host) = mpsc::channel(1);
    let Transport::Http { url, oauth, .. } = server.transport(None) else { unreachable!() };
    let headers = HeaderMap::from_iter([(AUTHORIZATION, HeaderValue::from_static("Bearer static"))]);

    let error = McpClient::connect(
        "linear",
        Transport::Http { url, headers, oauth },
        &ClientOptions::default().elicitation(events),
    )
    .await
    .err()
    .expect("unauthorized connect fails");

    assert!(matches!(error, McpError::Connect { server, .. } if server == "linear"));
}

#[tokio::test]
async fn oauth_is_not_offered_without_url_elicitation() {
    let server = FakeAuthServer::start().await;
    let (events, _host) = mpsc::channel(1);
    let options = ClientOptions::default()
        .elicitation(events)
        .elicitation_capability(ElicitationCapability::new().with_form(FormElicitationCapability::new()));

    let error = McpClient::connect("linear", server.transport(None), &options).await.err().expect("connect fails");

    assert!(matches!(error, McpError::Connect { .. }));
}

#[tokio::test]
async fn authorize_requires_an_event_sink() {
    let server = FakeAuthServer::start().await;

    let error = McpClient::authorize("linear", server.transport(None), &ClientOptions::default(), None)
        .await
        .err()
        .expect("authorization needs a host to present the URL");

    assert!(matches!(error, McpError::OAuthUnavailable { server } if server == "linear"));
}

#[tokio::test]
async fn authorization_presents_the_default_client_metadata_and_a_loopback_redirect() {
    let server = FakeAuthServer::start().await;
    let mut flow = AuthFlow::start(server.transport(None));

    let prompt = flow.prompt().await;
    let (client_id, redirect_uri) = (prompt.query("client_id"), prompt.query("redirect_uri"));
    prompt.elicitation.respond(ElicitResult::new(ElicitationAction::Decline));

    assert_eq!(client_id, CLIENT_METADATA_URL);
    assert!(redirect_uri.starts_with("http://localhost:"), "{redirect_uri}");
    assert!(matches!(
        flow.finish().await,
        Err(McpError::Auth { server, source: OAuthError::UserCancelled }) if server == "linear"
    ));
}

#[tokio::test]
async fn a_preregistered_client_id_takes_priority_over_client_metadata() {
    let server = FakeAuthServer::start().await;
    let oauth = McpOAuthConfig {
        client_id: Some("registered".to_string()),
        client_metadata_url: Some("https://client.example/oauth/client.json".to_string()),
        callback_port: None,
    };
    let mut flow = AuthFlow::start(server.transport(Some(oauth)));

    let prompt = flow.prompt().await;
    let client_id = prompt.query("client_id");
    drop(prompt);

    assert_eq!(client_id, "registered");
    assert!(matches!(flow.finish().await, Err(McpError::Auth { source: OAuthError::UserCancelled, .. })));
}

#[tokio::test]
async fn accepted_authorization_waits_for_the_callback_then_connects_with_the_token() {
    let server = FakeAuthServer::start().await;
    let mut flow = AuthFlow::start(server.transport(None));

    let prompt = flow.prompt().await;
    let callback = prompt.callback_url();
    prompt.elicitation.respond(ElicitResult::new(ElicitationAction::Accept));
    yield_now().await;
    assert!(!flow.task.is_finished(), "accepting the prompt does not complete the browser flow");
    reqwest::get(callback).await.expect("loopback callback responds");

    assert!(matches!(
        flow.host.recv().await,
        Some(Elicitation::Complete { server, id }) if server == "linear" && id == "oauth"
    ));
    let client = flow.finish().await.expect("the token authorizes the MCP handshake");
    assert!(client.list_tools().await.unwrap().iter().any(|tool| tool.name == "add_numbers"));
    assert!(server.received_bearer("access-token"));
}

#[tokio::test]
async fn gateway_servers_that_demand_oauth_need_it() {
    let server = FakeAuthServer::start().await;
    let (gateway, _host) = oauth_gateway();

    gateway.add_servers(vec![server.spec("linear")]).unwrap();
    let catalog = gateway.ready().await;

    let status = &catalog.statuses()[0];
    assert_eq!(status.status, McpServerStatus::NeedsOAuth);
    assert!(status.can_authenticate());
    assert!(catalog.tools().is_empty());
    assert!(catalog.instructions().is_empty());
}

#[tokio::test]
async fn gateway_servers_that_demand_oauth_fail_without_url_elicitation() {
    let server = FakeAuthServer::start().await;
    let (events, _host) = mpsc::channel(1);
    let options = ClientOptions::default()
        .elicitation(events)
        .elicitation_capability(ElicitationCapability::new().with_form(FormElicitationCapability::new()));
    let gateway = McpGateway::new(options, ToolFilter::default());

    gateway.add_servers(vec![server.spec("linear")]).unwrap();

    let status = &gateway.ready().await.statuses()[0];
    assert!(matches!(status.status, McpServerStatus::Failed { .. }));
    assert!(!status.can_authenticate());
}

#[tokio::test]
async fn gateway_authentication_reconnects_the_server_once_authorized() {
    let server = FakeAuthServer::start().await;
    let (gateway, mut host) = oauth_gateway();
    gateway.add_servers(vec![server.spec("linear")]).unwrap();
    gateway.ready().await;

    let authentication = authenticate(&gateway, "linear");
    let prompt = next_prompt(&mut host).await;
    let callback = prompt.callback_url();
    prompt.elicitation.respond(ElicitResult::new(ElicitationAction::Accept));
    reqwest::get(callback).await.expect("loopback callback responds");

    authentication.await.unwrap().expect("authentication succeeds");
    let catalog = gateway.catalog();
    assert_eq!(catalog.statuses()[0].status, McpServerStatus::Connected { tool_count: 3 });
    assert!(catalog.statuses()[0].can_authenticate());
    assert!(catalog.tools().iter().any(|tool| tool.name == "linear__add_numbers"));
    assert!(matches!(host.recv().await, Some(Elicitation::Complete { server, .. }) if server == "linear"));
}

#[tokio::test]
async fn gateway_authentication_reports_a_declined_authorization_and_allows_a_retry() {
    let server = FakeAuthServer::start().await;
    let (gateway, mut host) = oauth_gateway();
    gateway.add_servers(vec![server.spec("linear")]).unwrap();
    gateway.ready().await;
    let mut catalog = gateway.subscribe();

    let authentication = authenticate(&gateway, "linear");
    catalog.wait_for(|catalog| catalog.statuses()[0].status == McpServerStatus::Authenticating).await.unwrap();
    next_prompt(&mut host).await.elicitation.respond(ElicitResult::new(ElicitationAction::Decline));

    let declined = authentication.await.unwrap();
    assert!(
        matches!(declined, Err(McpError::Auth { server, source: OAuthError::UserCancelled }) if server == "linear")
    );
    let status = gateway.catalog().statuses().remove(0);
    assert!(matches!(status.status, McpServerStatus::Failed { .. }));
    assert!(status.can_authenticate());
    let retry = authenticate(&gateway, "linear");
    drop(next_prompt(&mut host).await);
    assert!(matches!(retry.await.unwrap(), Err(McpError::Auth { source: OAuthError::UserCancelled, .. })));
}

#[tokio::test]
async fn a_configured_callback_port_that_is_taken_fails_authorization() {
    let server = FakeAuthServer::start().await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = NonZeroU16::new(taken.local_addr().unwrap().port());
    let oauth = McpOAuthConfig { callback_port: port, ..McpOAuthConfig::default() };
    let (events, _host) = mpsc::channel(1);

    let error = McpClient::authorize(
        "linear",
        server.transport(Some(oauth)),
        &ClientOptions::default().elicitation(events),
        None,
    )
    .await
    .err()
    .expect("the callback port is in use");

    assert!(matches!(
        error,
        McpError::Auth { source: OAuthError::Io(error), .. } if error.kind() == std::io::ErrorKind::AddrInUse
    ));
}

#[test]
fn oauth_settings_expand_variables() {
    let vars = Vars::new().with("CLIENT_ID", "registered").with("CIMD_URL", "https://client.example/oauth/client.json");
    let config: RemoteServerConfig = serde_json::from_value(json!({
        "type": "http",
        "url": "https://example.com/mcp",
        "oauth": { "clientId": "$CLIENT_ID", "clientMetadataUrl": "$CIMD_URL", "callbackPort": 4000 }
    }))
    .unwrap();

    let Transport::Http { oauth: Some(oauth), .. } = config.into_transport(&vars).unwrap() else {
        panic!("expected HTTP transport with OAuth settings")
    };
    assert_eq!(oauth.client_id.as_deref(), Some("registered"));
    assert_eq!(oauth.client_metadata_url.as_deref(), Some("https://client.example/oauth/client.json"));
    assert_eq!(oauth.callback_port.map(NonZeroU16::get), Some(4000));
}

struct AuthFlow {
    task: JoinHandle<Result<McpClient, McpError>>,
    host: mpsc::Receiver<Elicitation>,
}

struct Prompt {
    elicitation: Box<ElicitationRequest>,
    url: Url,
}

impl AuthFlow {
    fn start(transport: Transport) -> Self {
        let (events, host) = mpsc::channel(4);
        let options = oauth_options().elicitation(events);
        let task = tokio::spawn(async move { McpClient::authorize("linear", transport, &options, None).await });
        Self { task, host }
    }

    async fn prompt(&mut self) -> Prompt {
        next_prompt(&mut self.host).await
    }

    async fn finish(self) -> Result<McpClient, McpError> {
        self.task.await.unwrap()
    }
}

impl Prompt {
    fn query(&self, name: &str) -> String {
        self.url.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.into_owned()).unwrap_or_default()
    }

    fn callback_url(&self) -> String {
        let mut callback = Url::parse(&self.query("redirect_uri")).unwrap();
        callback.query_pairs_mut().append_pair("code", "test-code").append_pair("state", &self.query("state"));
        callback.to_string()
    }
}

const CLIENT_METADATA_URL: &str = "https://client.example/oauth/client-metadata.json";

fn oauth_options() -> ClientOptions {
    ClientOptions::default().oauth_client_metadata_url(CLIENT_METADATA_URL)
}

fn oauth_gateway() -> (McpGateway, mpsc::Receiver<Elicitation>) {
    let (events, host) = mpsc::channel(4);
    (McpGateway::new(oauth_options().elicitation(events), ToolFilter::default()), host)
}

fn authenticate(gateway: &McpGateway, server: &str) -> JoinHandle<Result<(), McpError>> {
    let (gateway, server) = (gateway.clone(), server.to_string());
    tokio::spawn(async move { gateway.authenticate(&server).await })
}

async fn next_prompt(host: &mut mpsc::Receiver<Elicitation>) -> Prompt {
    let Some(Elicitation::Request(elicitation)) = host.recv().await else {
        panic!("expected the authorization prompt")
    };
    assert_eq!(elicitation.server, "linear");
    let ElicitRequestParams::UrlElicitationParams { url, elicitation_id, .. } = &elicitation.request else {
        panic!("expected a URL elicitation")
    };
    assert_eq!(elicitation_id, "oauth");
    let url = Url::parse(url).unwrap();
    Prompt { elicitation, url }
}

struct FakeAuthServer {
    url: String,
    auth: Arc<AuthState>,
    task: JoinHandle<()>,
}

struct AuthState {
    origin: String,
    bearers: Mutex<Vec<String>>,
}

impl FakeAuthServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let auth = Arc::new(AuthState { origin: origin.clone(), bearers: Mutex::new(Vec::new()) });
        let mcp = StreamableHttpService::new(
            || Ok(FakeMcpServer::new()),
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
            axum::serve(listener, app).await.unwrap();
        });
        Self { url: format!("{origin}/mcp"), auth, task }
    }

    fn transport(&self, oauth: Option<McpOAuthConfig>) -> Transport {
        Transport::Http { url: self.url.clone(), headers: HeaderMap::new(), oauth }
    }

    fn spec(&self, name: &str) -> ServerSpec {
        ServerSpec { name: name.to_string(), transport: self.transport(None), exposure: ToolExposure::ModelVisible }
    }

    fn received_bearer(&self, token: &str) -> bool {
        self.auth.bearers.lock().unwrap().iter().any(|bearer| bearer == token)
    }
}

impl Drop for FakeAuthServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

const ACCESS_TOKEN: &str = "access-token";

async fn require_token(State(auth): State<Arc<AuthState>>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string);
    if let Some(bearer) = bearer {
        auth.bearers.lock().unwrap().push(bearer.clone());
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
