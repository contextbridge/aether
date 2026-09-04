use std::time::SystemTime;

use aws_credential_types::provider::{ProvideCredentials, SharedCredentialsProvider};
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use aws_smithy_runtime_api::client::identity::Identity;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};

use crate::{LlmError, Result};

/// Credential scheme used to authenticate a Mantle request.
///
/// Bedrock accepts either a long-lived API key presented as a bearer token or a
/// `SigV4` signature derived from the standard AWS credential chain.
#[derive(Clone)]
pub enum MantleAuth {
    /// `AWS_BEARER_TOKEN_BEDROCK`, or a token supplied programmatically.
    BearerToken(String),
    /// `SigV4` over credentials resolved from the AWS credential chain.
    SigV4 { credentials: SharedCredentialsProvider, region: String },
    /// Send the request unsigned.
    None,
}

pub const BEARER_TOKEN_ENV_VAR: &str = "AWS_BEARER_TOKEN_BEDROCK";
const SIGNING_SERVICE: &str = "bedrock";

impl MantleAuth {
    /// Read a bearer token from the environment, ignoring an empty value.
    pub fn bearer_token_from_env() -> Option<String> {
        std::env::var(BEARER_TOKEN_ENV_VAR).ok().filter(|token| !token.is_empty())
    }

    pub async fn headers(&self, method: &str, url: &str, body: &[u8]) -> Result<HeaderMap> {
        match self {
            Self::BearerToken(token) => {
                let mut headers = HeaderMap::new();
                let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
                    .map_err(|e| LlmError::ProviderRequest(e.to_string()))?;
                value.set_sensitive(true);
                headers.insert(AUTHORIZATION, value);
                Ok(headers)
            }
            Self::SigV4 { credentials, region } => sigv4_headers(credentials, region, method, url, body).await,
            Self::None => Ok(HeaderMap::new()),
        }
    }
}

impl std::fmt::Display for MantleAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BearerToken(_) => "bearer token",
            Self::SigV4 { .. } => "SigV4",
            Self::None => "unsigned",
        })
    }
}

async fn sigv4_headers(
    credentials: &SharedCredentialsProvider,
    region: &str,
    method: &str,
    url: &str,
    body: &[u8],
) -> Result<HeaderMap> {
    let credentials = credentials
        .provide_credentials()
        .await
        .map_err(|e| LlmError::ProviderRequest(format!("Could not resolve AWS credentials: {e}")))?;
    let identity = Identity::from(credentials);

    let params = v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(SIGNING_SERVICE)
        .time(SystemTime::now())
        .settings(SigningSettings::default())
        .build()
        .map_err(|e| LlmError::ProviderRequest(format!("Could not build SigV4 signing params: {e}")))?
        .into();

    let signable = SignableRequest::new(method, url, std::iter::empty(), SignableBody::Bytes(body))
        .map_err(|e| LlmError::ProviderRequest(format!("Could not build signable request: {e}")))?;

    let (instructions, _signature) = sign(signable, &params)
        .map_err(|e| LlmError::ProviderRequest(format!("SigV4 signing failed: {e}")))?
        .into_parts();

    let mut headers = HeaderMap::new();
    for (name, value) in instructions.headers() {
        let name = HeaderName::try_from(name)
            .map_err(|e| LlmError::ProviderRequest(format!("Invalid signed header name: {e}")))?;
        let mut value = HeaderValue::from_str(value)
            .map_err(|e| LlmError::ProviderRequest(format!("Invalid signed header value: {e}")))?;
        value.set_sensitive(true);
        headers.insert(name, value);
    }
    Ok(headers)
}
