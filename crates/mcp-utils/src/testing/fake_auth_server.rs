use super::fake_http_server::origin;
use super::{FakeHttpServer, FakeMcpServer, RecordedRequest};
use crate::client::{Elicitation, ElicitationRequest, HttpAuth, OAuthSettings, Transport};
use crate::gateway::ServerSpec;
use aether_auth::OAuthClientRegistration;
use axum::http::{StatusCode, Uri, header::WWW_AUTHENTICATE};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap};
use rmcp::model::{ElicitRequestParams, ElicitResult, ElicitationAction};
use serde_json::{Value, json};
use tokio::sync::mpsc;

pub struct FakeAuthServer {
    http: FakeHttpServer,
}

pub struct OAuthPrompt {
    pub elicitation: Box<ElicitationRequest>,
    pub url: Url,
}

pub const CLIENT_METADATA_URL: &str = "https://client.example/oauth/client-metadata.json";

impl FakeAuthServer {
    pub async fn start(server: FakeMcpServer) -> Self {
        let routes = Router::new().route("/token", post(token)).fallback(metadata);
        Self { http: FakeHttpServer::serve(server, routes, require_token).await }
    }

    pub fn transport(&self) -> Transport {
        self.transport_with(HttpAuth::OAuth(client_metadata()))
    }

    pub fn transport_with(&self, auth: HttpAuth) -> Transport {
        self.http.transport(HeaderMap::new(), auth)
    }

    pub fn spec(&self, name: &str) -> ServerSpec {
        ServerSpec { name: name.to_string(), transport: self.transport() }
    }

    pub fn received_bearer(&self, token: &str) -> bool {
        let bearer = format!("Bearer {token}");
        self.http
            .requests()
            .iter()
            .any(|request| request.headers.get(AUTHORIZATION).is_some_and(|value| *value == bearer))
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

fn require_token(request: &RecordedRequest) -> Result<(), impl IntoResponse + use<>> {
    let bearer = format!("Bearer {ACCESS_TOKEN}");
    if request.headers.get(AUTHORIZATION).is_some_and(|value| *value == bearer) {
        return Ok(());
    }
    let challenge =
        format!("Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\"", origin(&request.headers));
    Err((StatusCode::UNAUTHORIZED, [(WWW_AUTHENTICATE, challenge)]))
}

async fn token() -> Json<Value> {
    Json(json!({ "access_token": ACCESS_TOKEN, "token_type": "Bearer", "expires_in": 3600 }))
}

async fn metadata(headers: HeaderMap, uri: Uri) -> Json<Value> {
    let origin = origin(&headers);
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
