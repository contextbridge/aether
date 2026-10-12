use aether_auth::{SigV4Config, SigV4Credentials, SigV4Signer};
use aws_credential_types::provider::SharedCredentialsProvider;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use std::time::Duration;

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
    SigV4(SigV4Signer),
    /// Send the request unsigned.
    None,
}

pub const BEARER_TOKEN_ENV_VAR: &str = "AWS_BEARER_TOKEN_BEDROCK";
const SIGNING_SERVICE: &str = "bedrock";

impl MantleAuth {
    pub fn sigv4(credentials: SharedCredentialsProvider, region: &str) -> Self {
        Self::SigV4(SigV4Signer::new(SigV4Config {
            service: SIGNING_SERVICE.to_string(),
            region: region.to_string(),
            credentials: SigV4Credentials::Provider(credentials),
            refresh_before_expiry: Duration::from_mins(5),
        }))
    }

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
            Self::SigV4(signer) => {
                signer.headers(method, url, body).await.map_err(|e| LlmError::ProviderRequest(e.to_string()))
            }
            Self::None => Ok(HeaderMap::new()),
        }
    }
}

impl std::fmt::Display for MantleAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BearerToken(_) => "bearer token",
            Self::SigV4(_) => "SigV4",
            Self::None => "unsigned",
        })
    }
}
