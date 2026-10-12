use aether_auth::{SigV4Config, SigV4Credentials, SigV4Signer};
use aws_credential_types::Credentials;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use axum::http::{Method, StatusCode, header::AUTHORIZATION};
use chrono::NaiveDateTime;
use mcp_utils::client::{ClientError, ClientOptions, HeaderMap, HttpAuth, McpClient, Transport};
use mcp_utils::testing::{FakeHttpServer, FakeMcpServer, FakeTool, FakeToolResponse, RecordedRequest, call};
use rmcp::ServerHandler;
use serde_json::json;
use std::time::{Duration, SystemTime};

#[tokio::test]
async fn tool_calls_are_signed_with_the_configured_credentials() {
    let server = FakeMcpServer::new().with_tool(FakeTool::new("echo").responds(FakeToolResponse::text("ok")));
    let gateway = sigv4_gateway(server, credentials("secret")).await;
    let client = McpClient::connect("gateway", transport(&gateway, credentials("secret")), &ClientOptions::default())
        .await
        .unwrap();

    let result = call(&client, "echo", json!({ "text": "héllo \"quoted\" & <escaped>" })).await;

    assert!(result.is_ok(), "the signed tool call was rejected: {result:?}");
}

#[tokio::test]
async fn every_request_of_a_legacy_session_is_signed() {
    let gateway = sigv4_gateway(FakeMcpServer::new().with_legacy_protocol(), credentials("secret")).await;
    let client = McpClient::connect("gateway", transport(&gateway, credentials("secret")), &ClientOptions::default())
        .await
        .unwrap();

    let requests = gateway.record_session(client).await;

    for method in [Method::POST, Method::GET, Method::DELETE] {
        assert!(requests.iter().any(|request| request.method == method), "the client sent no {method}");
    }
    let unsigned = requests.iter().find(|request| !signed_by(&credentials("secret"), request));
    assert!(unsigned.is_none(), "the client sent an unsigned request: {unsigned:?}");
}

#[tokio::test]
async fn requests_signed_with_the_wrong_secret_are_rejected() {
    let gateway = sigv4_gateway(FakeMcpServer::new(), credentials("secret")).await;

    let result =
        McpClient::connect("gateway", transport(&gateway, credentials("wrong")), &ClientOptions::default()).await;

    assert!(matches!(result, Err(ClientError::Connect { .. })), "expected the connection to be rejected");
}

const SERVICE: &str = "execute-api";
const REGION: &str = "us-east-1";

async fn sigv4_gateway(server: impl ServerHandler + Clone, credentials: Credentials) -> FakeHttpServer {
    FakeHttpServer::start_guarded(server, move |request| {
        signed_by(&credentials, request).then_some(()).ok_or(StatusCode::FORBIDDEN)
    })
    .await
}

fn transport(gateway: &FakeHttpServer, credentials: Credentials) -> Transport {
    let signer = SigV4Signer::new(SigV4Config {
        service: SERVICE.to_string(),
        region: REGION.to_string(),
        credentials: SigV4Credentials::Provider(SharedCredentialsProvider::new(credentials)),
        refresh_before_expiry: Duration::from_mins(5),
    });
    gateway.transport(HeaderMap::new(), HttpAuth::SigV4(signer))
}

fn signed_by(credentials: &Credentials, request: &RecordedRequest) -> bool {
    let header = |name: &str| request.headers.get(name).and_then(|value| value.to_str().ok());
    header(AUTHORIZATION.as_str())
        .zip(header("x-amz-date"))
        .is_some_and(|(authorization, date)| authorization == expected_authorization(credentials, request, date))
}

fn expected_authorization(credentials: &Credentials, request: &RecordedRequest, date: &str) -> String {
    let time: SystemTime = NaiveDateTime::parse_from_str(date, "%Y%m%dT%H%M%SZ").unwrap().and_utc().into();
    let identity = credentials.clone().into();
    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(REGION)
        .name(SERVICE)
        .time(time)
        .settings(SigningSettings::default())
        .build()
        .unwrap()
        .into();
    let body = SignableBody::Bytes(&request.body);
    let signable = SignableRequest::new(request.method.as_str(), &request.url, std::iter::empty(), body).unwrap();
    let (instructions, _) = sign(signable, &params).unwrap().into_parts();
    instructions.headers().find(|(name, _)| name.eq_ignore_ascii_case("authorization")).unwrap().1.to_string()
}

fn credentials(secret: &str) -> Credentials {
    Credentials::new("AKIDEXAMPLE", secret, None, None, "test")
}
