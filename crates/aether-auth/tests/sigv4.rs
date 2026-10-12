#![cfg(feature = "sigv4")]

use aether_auth::{SigV4Config, SigV4Credentials, SigV4Error, SigV4Signer};
use aws_credential_types::Credentials;
use aws_credential_types::credential_fn::provide_credentials_fn;
use aws_credential_types::provider::error::CredentialsError;
use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use reqwest::header::HeaderValue;
use std::future::ready;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

#[tokio::test]
async fn credentials_are_reused_while_they_are_fresh() {
    let signer = signer(rotating(Duration::from_hours(1)));
    let first = signing_key(&signer).await;
    let second = signing_key(&signer).await;
    assert_eq!([first, second], ["AKID1", "AKID1"]);
}

#[tokio::test]
async fn credentials_expiring_within_the_refresh_window_are_refreshed() {
    let signer = SigV4Signer::new(SigV4Config {
        refresh_before_expiry: Duration::from_hours(2),
        ..config(rotating(Duration::from_hours(1)))
    });

    let first = signing_key(&signer).await;
    let second = signing_key(&signer).await;
    assert_eq!([first, second], ["AKID1", "AKID2"]);
}

#[tokio::test]
async fn clones_share_resolved_credentials() {
    let signer = signer(rotating(Duration::from_hours(1)));
    let original = signing_key(&signer).await;
    let clone = signing_key(&signer.clone()).await;
    assert_eq!([original, clone], ["AKID1", "AKID1"]);
}

#[tokio::test]
async fn signed_headers_carry_the_session_token_and_are_sensitive() {
    let credentials = Credentials::new("AKID", "secret", Some("session".to_string()), None, "test");
    let signer = signer(credentials);
    let headers = signer.headers("POST", "https://example.com/mcp", b"{}").await.unwrap();

    assert_eq!(headers["x-amz-security-token"], "session");
    assert!(headers.contains_key("x-amz-date"));
    assert!(headers.values().all(HeaderValue::is_sensitive), "{headers:?}");
}

#[tokio::test]
async fn credential_failures_explain_why() {
    let signer = signer(missing("the SSO session has expired"));
    let error = signer.headers("GET", "https://example.com/mcp", &[]).await.unwrap_err();

    assert!(matches!(error, SigV4Error::Credentials(_)));
    assert!(error.to_string().contains("the SSO session has expired"), "{error}");
}

fn rotating(lifetime: Duration) -> impl ProvideCredentials + 'static {
    let issued = AtomicUsize::new(0);
    provide_credentials_fn(move || {
        let issued = issued.fetch_add(1, Ordering::SeqCst) + 1;
        let expiry = SystemTime::now() + lifetime;
        ready(Ok(Credentials::new(format!("AKID{issued}"), "secret", None, Some(expiry), "sts")))
    })
}

fn missing(reason: &'static str) -> impl ProvideCredentials + 'static {
    provide_credentials_fn(move || ready(Err(CredentialsError::provider_error(reason))))
}

fn signer(credentials: impl ProvideCredentials + 'static) -> SigV4Signer {
    SigV4Signer::new(config(credentials))
}

fn config(credentials: impl ProvideCredentials + 'static) -> SigV4Config {
    SigV4Config {
        service: "execute-api".to_string(),
        region: "us-east-1".to_string(),
        credentials: SigV4Credentials::Provider(SharedCredentialsProvider::new(credentials)),
        refresh_before_expiry: Duration::from_mins(5),
    }
}

async fn signing_key(signer: &SigV4Signer) -> String {
    let headers = signer.headers("POST", "https://example.com/mcp", b"{}").await.unwrap();
    let authorization = headers["authorization"].to_str().unwrap();
    let credential = authorization.split("Credential=").nth(1).expect("a credential scope");
    credential.split('/').next().unwrap().to_string()
}
