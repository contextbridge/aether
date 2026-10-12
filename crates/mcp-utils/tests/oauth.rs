use aether_auth::{OAuthClientRegistration, OAuthError};
use mcp_utils::client::{
    ClientError, ClientOptions, Elicitation, ElicitationSink, HttpAuth, McpClient, OAuthSettings, Transport,
};
use mcp_utils::gateway::{GatewayError, McpGateway, ServerState};
use mcp_utils::model::{ElicitResult, ElicitationAction, ElicitationCapability, FormElicitationCapability};
use mcp_utils::testing::{
    ACCESS_TOKEN, CLIENT_METADATA_URL, FakeAuthServer, FakeMcpServer, OAuthPrompt, client_metadata, elicitation_host,
    wait_until,
};
use tokio::sync::mpsc;
use tokio::task::{JoinHandle, yield_now};

#[tokio::test]
async fn connect_reports_the_authorization_challenge() {
    let server = math_auth_server().await;
    let (options, _host) = elicitation_host();

    let error =
        McpClient::connect("linear", server.transport(), &options).await.expect_err("unauthorized connect fails");

    assert!(matches!(
        error,
        ClientError::AuthRequired { server, challenge: Some(challenge) }
            if server == "linear" && challenge.contains("resource_metadata")
    ));
}

#[tokio::test]
async fn servers_without_oauth_settings_are_not_offered_oauth() {
    let server = math_auth_server().await;
    let (options, _host) = elicitation_host();

    let connected = McpClient::connect("linear", server.transport_with(HttpAuth::None), &options).await;
    let authorized = McpClient::authorize("linear", server.transport_with(HttpAuth::None), &options, None).await;

    assert!(matches!(connected, Err(ClientError::Connect { server, .. }) if server == "linear"));
    assert!(matches!(authorized, Err(ClientError::OAuthUnavailable { server }) if server == "linear"));
}

#[tokio::test]
async fn oauth_is_not_offered_without_url_elicitation() {
    let server = math_auth_server().await;
    let (events, _host) = mpsc::channel(1);
    let options = ClientOptions::default().elicitation(ElicitationSink::new(
        events,
        ElicitationCapability::new().with_form(FormElicitationCapability::new()),
    ));

    let error = McpClient::connect("linear", server.transport(), &options).await.expect_err("connect fails");

    assert!(matches!(error, ClientError::Connect { .. }));
}

#[tokio::test]
async fn authorize_requires_an_event_sink() {
    let server = math_auth_server().await;

    let error = McpClient::authorize("linear", server.transport(), &ClientOptions::default(), None)
        .await
        .expect_err("authorization needs a host to present the URL");

    assert!(matches!(error, ClientError::OAuthUnavailable { server } if server == "linear"));
}

#[tokio::test]
async fn authorization_presents_the_client_metadata_and_a_loopback_redirect() {
    let server = math_auth_server().await;
    let mut flow = AuthFlow::start(server.transport());

    let prompt = flow.prompt().await;
    let (client_id, redirect_uri) = (prompt.query("client_id"), prompt.query("redirect_uri"));
    prompt.elicitation.respond(ElicitResult::new(ElicitationAction::Decline));

    assert_eq!(client_id, CLIENT_METADATA_URL);
    assert!(redirect_uri.starts_with("http://localhost:"), "{redirect_uri}");
    assert!(matches!(
        flow.finish().await,
        Err(ClientError::Auth { server, source: OAuthError::UserCancelled }) if server == "linear"
    ));
}

#[tokio::test]
async fn authorization_redirects_to_the_configured_callback_port() {
    let server = math_auth_server().await;
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut flow = AuthFlow::start(
        server.transport_with(HttpAuth::OAuth(OAuthSettings { callback_port: port, ..client_metadata() })),
    );

    let prompt = flow.prompt().await;
    let redirect_uri = prompt.query("redirect_uri");
    drop(prompt);

    assert_eq!(redirect_uri, format!("http://localhost:{port}/"));
    assert!(matches!(flow.finish().await, Err(ClientError::Auth { source: OAuthError::UserCancelled, .. })));
}

#[tokio::test]
async fn authorization_presents_a_preregistered_client_id() {
    let server = math_auth_server().await;
    let oauth = OAuthSettings {
        registration: OAuthClientRegistration::PreRegistered("registered".to_string()),
        callback_port: 0,
    };
    let mut flow = AuthFlow::start(server.transport_with(HttpAuth::OAuth(oauth)));

    let prompt = flow.prompt().await;
    let client_id = prompt.query("client_id");
    drop(prompt);

    assert_eq!(client_id, "registered");
    assert!(matches!(flow.finish().await, Err(ClientError::Auth { source: OAuthError::UserCancelled, .. })));
}

#[tokio::test]
async fn accepted_authorization_waits_for_the_callback_then_connects_with_the_token() {
    let server = math_auth_server().await;
    let mut flow = AuthFlow::start(server.transport());

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
    assert!(server.received_bearer(ACCESS_TOKEN));
}

#[tokio::test]
async fn gateway_servers_that_demand_oauth_need_it() {
    let server = math_auth_server().await;
    let (gateway, _host) = oauth_gateway(&server);

    gateway.ready().await;

    let status = &gateway.statuses()[0];
    assert_eq!(status.state, ServerState::NeedsOAuth);
    assert!(status.can_sign_in);
    assert!(gateway.servers().is_empty());
}

#[tokio::test]
async fn gateway_servers_that_demand_oauth_fail_without_url_elicitation() {
    let server = math_auth_server().await;
    let (events, _host) = mpsc::channel(1);
    let options = ClientOptions::default().elicitation(ElicitationSink::new(
        events,
        ElicitationCapability::new().with_form(FormElicitationCapability::new()),
    ));
    let gateway = McpGateway::new(vec![server.spec("linear")], &options);

    gateway.ready().await;
    let status = &gateway.statuses()[0];
    assert!(matches!(status.state, ServerState::Failed { .. }));
    assert!(!status.can_sign_in);
}

#[tokio::test]
async fn gateway_authentication_reconnects_the_server_once_authorized() {
    let server = math_auth_server().await;
    let (gateway, mut host) = oauth_gateway(&server);
    gateway.ready().await;

    let authentication = authenticate(&gateway, "linear");
    let prompt = next_prompt(&mut host).await;
    let callback = prompt.callback_url();
    prompt.elicitation.respond(ElicitResult::new(ElicitationAction::Accept));
    reqwest::get(callback).await.expect("loopback callback responds");

    authentication.await.unwrap().expect("authentication succeeds");
    assert_eq!(gateway.statuses()[0].state, ServerState::Connected { tool_count: 2 });
    assert!(gateway.statuses()[0].can_sign_in);
    assert!(gateway.servers()[0].tools().iter().any(|tool| tool.name == "add_numbers"));
    assert!(matches!(host.recv().await, Some(Elicitation::Complete { server, .. }) if server == "linear"));
}

#[tokio::test]
async fn gateway_authentication_reports_a_declined_authorization_and_allows_a_retry() {
    let server = math_auth_server().await;
    let (gateway, mut host) = oauth_gateway(&server);
    gateway.ready().await;

    let authentication = authenticate(&gateway, "linear");
    wait_until(&gateway, |gateway| gateway.statuses()[0].state == ServerState::Authenticating).await;
    next_prompt(&mut host).await.elicitation.respond(ElicitResult::new(ElicitationAction::Decline));

    let declined = authentication.await.unwrap();
    assert!(
        matches!(declined, Err(GatewayError::Client(ClientError::Auth { server, source: OAuthError::UserCancelled })) if server == "linear")
    );
    let status = gateway.statuses().remove(0);
    assert!(matches!(status.state, ServerState::Failed { .. }));
    assert!(status.can_sign_in);
    let retry = authenticate(&gateway, "linear");
    drop(next_prompt(&mut host).await);
    assert!(matches!(
        retry.await.unwrap(),
        Err(GatewayError::Client(ClientError::Auth { source: OAuthError::UserCancelled, .. }))
    ));
}

#[tokio::test]
async fn authenticating_again_cancels_the_pending_authentication() {
    let server = math_auth_server().await;
    let (gateway, mut host) = oauth_gateway(&server);
    gateway.ready().await;
    let authentication = authenticate(&gateway, "linear");
    let _prompt = next_prompt(&mut host).await;

    let _again = authenticate(&gateway, "linear");

    let cancelled = authentication.await.unwrap();
    assert!(
        matches!(cancelled, Err(GatewayError::Client(ClientError::Auth { server, source: OAuthError::UserCancelled })) if server == "linear")
    );
}

#[tokio::test]
async fn shutting_down_abandons_a_pending_authentication() {
    let server = math_auth_server().await;
    let (gateway, mut host) = oauth_gateway(&server);
    gateway.ready().await;
    let authentication = authenticate(&gateway, "linear");
    let _prompt = next_prompt(&mut host).await;

    gateway.shutdown().await;

    assert!(matches!(authentication.await.unwrap(), Err(GatewayError::Closed)));
}

#[tokio::test]
async fn a_configured_callback_port_that_is_taken_fails_authorization() {
    let server = math_auth_server().await;
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let oauth = OAuthSettings { callback_port: taken.local_addr().unwrap().port(), ..client_metadata() };
    let (options, _host) = elicitation_host();

    let error = McpClient::authorize("linear", server.transport_with(HttpAuth::OAuth(oauth)), &options, None)
        .await
        .expect_err("the callback port is in use");

    assert!(matches!(
        error,
        ClientError::Auth { source: OAuthError::Io(error), .. } if error.kind() == std::io::ErrorKind::AddrInUse
    ));
}

struct AuthFlow {
    task: JoinHandle<Result<McpClient, ClientError>>,
    host: mpsc::Receiver<Elicitation>,
}

impl AuthFlow {
    fn start(transport: Transport) -> Self {
        let (options, host) = elicitation_host();
        let task = tokio::spawn(async move { McpClient::authorize("linear", transport, &options, None).await });
        Self { task, host }
    }

    async fn prompt(&mut self) -> OAuthPrompt {
        next_prompt(&mut self.host).await
    }

    async fn finish(self) -> Result<McpClient, ClientError> {
        self.task.await.unwrap()
    }
}

fn oauth_gateway(server: &FakeAuthServer) -> (McpGateway, mpsc::Receiver<Elicitation>) {
    let (options, host) = elicitation_host();
    (McpGateway::new(vec![server.spec("linear")], &options), host)
}

fn authenticate(gateway: &McpGateway, server: &str) -> JoinHandle<Result<(), GatewayError>> {
    let (gateway, server) = (gateway.clone(), server.to_string());
    tokio::spawn(async move { gateway.authenticate(&server).await })
}

async fn next_prompt(host: &mut mpsc::Receiver<Elicitation>) -> OAuthPrompt {
    let prompt = OAuthPrompt::receive(host).await;
    assert_eq!(prompt.elicitation.server, "linear");
    prompt
}

async fn math_auth_server() -> FakeAuthServer {
    FakeAuthServer::start(FakeMcpServer::math()).await
}
