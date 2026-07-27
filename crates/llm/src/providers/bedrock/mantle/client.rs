use reqwest::header::{ACCEPT, CONTENT_TYPE, HeaderValue};
use tracing::{debug, error};

use super::auth::MantleAuth;
use crate::catalog::transport::{ModelTransport, expand_api_template};
use crate::providers::openai_responses::mappers::{ResponsesRequestPolicy, build_wire_request};
use crate::providers::openai_responses::transport::{ResponsesEventStream, decode_response_sse};
use crate::{Context, LlmError, Result};

/// Transport for Bedrock models that serve the `OpenAI` Responses API rather than
/// the Converse API.
///
/// The endpoint is not fixed: each model's catalog entry carries its own
/// template (the `gpt-oss` models are served from `/v1` while the rest use
/// `/openai/v1`), so the URL is resolved per request from the model's
/// [`ModelTransport`].
#[derive(Clone)]
pub struct MantleClient {
    http: reqwest::Client,
    region: String,
    auth: MantleAuth,
    base_url_override: Option<String>,
}

impl MantleClient {
    pub fn new(region: String, auth: MantleAuth, base_url_override: Option<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            region,
            auth,
            base_url_override: base_url_override.map(|url| url.trim_end_matches('/').to_string()),
        }
    }

    pub fn with_auth(mut self, auth: MantleAuth) -> Self {
        self.auth = auth;
        self
    }

    /// Endpoint this client will POST to for `transport`.
    ///
    /// An explicit base URL — a test server or a private endpoint — wins over the
    /// catalog template, which is why the override is checked first.
    pub fn endpoint(&self, transport: &ModelTransport) -> Result<String> {
        let ModelTransport::OpenAiResponses { base_url_template } = transport;
        let base = match &self.base_url_override {
            Some(base) => base.clone(),
            None => self.expand(base_url_template)?,
        };

        Ok(format!("{}/responses", base.trim_end_matches('/')))
    }

    pub async fn stream(
        &self,
        model: &str,
        transport: &ModelTransport,
        context: &Context,
    ) -> Result<ResponsesEventStream> {
        let url = self.endpoint(transport)?;
        let body = serde_json::to_vec(&build_wire_request(model, context, &ResponsesRequestPolicy::mantle())?)?;

        debug!(model, url, auth = %self.auth, "Sending Bedrock Mantle responses request");

        let mut headers = self.auth.headers("POST", &url, &body).await?;
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));

        let response = self.http.post(&url).headers(headers).body(body).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            let message = format!("Bedrock Mantle request failed with status {status}: {error_text}");
            error!(model, %status, "Bedrock Mantle API error");

            return Err(match status.as_u16() {
                401 | 403 => LlmError::InvalidApiKey(message),
                429 => LlmError::RateLimited(message),
                s if (500..600).contains(&s) => LlmError::ServerError { status: Some(s), message },
                _ => LlmError::ApiError(message),
            });
        }

        Ok(decode_response_sse(response))
    }

    /// Resolve a catalog endpoint template. The region this client was built for
    /// answers the AWS region placeholders; anything else comes from the process
    /// environment.
    fn expand(&self, template: &str) -> Result<String> {
        expand_api_template(template, |name| match name {
            "AWS_REGION" | "AWS_DEFAULT_REGION" => Some(self.region.clone()),
            other => std::env::var(other).ok().filter(|value| !value.is_empty()),
        })
        .map_err(|e| LlmError::InvalidArgument(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn transport(api: &'static str) -> ModelTransport {
        ModelTransport::OpenAiResponses { base_url_template: api }
    }

    fn client(base_url_override: Option<String>) -> MantleClient {
        MantleClient::new("us-west-2".to_string(), MantleAuth::None, base_url_override)
    }

    #[test]
    fn endpoint_expands_the_region_from_the_model_template() {
        let endpoint =
            client(None).endpoint(&transport("https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1")).unwrap();

        assert_eq!(endpoint, "https://bedrock-mantle.us-west-2.api.aws/openai/v1/responses");
    }

    #[test]
    fn endpoint_preserves_a_models_own_api_path() {
        let endpoint = client(None).endpoint(&transport("https://bedrock-mantle.${AWS_REGION}.api.aws/v1")).unwrap();

        assert_eq!(endpoint, "https://bedrock-mantle.us-west-2.api.aws/v1/responses");
    }

    #[test]
    fn base_url_override_replaces_the_catalog_template() {
        let endpoint = client(Some("http://127.0.0.1:9999/".to_string()))
            .endpoint(&transport("https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1"))
            .unwrap();

        assert_eq!(endpoint, "http://127.0.0.1:9999/responses");
    }
}
