use tracing::debug;

use super::auth::MantleAuth;
use crate::catalog::transport::ModelTransport;
use crate::providers::http::{SseData, http_client};
use crate::providers::openai_responses::mappers::{ResponsesRequestPolicy, build_wire_request};
use crate::providers::openai_responses::transport::send;
use crate::providers::response_stream::OpenedStream;
use crate::{Context, LlmError, Result};
use utils::variables::Vars;

/// Transport for Bedrock models served through the `OpenAI` Responses API.
#[derive(Clone)]
pub struct MantleClient {
    http: reqwest::Client,
    template_vars: Vars,
    auth: MantleAuth,
    base_url_override: Option<String>,
}

impl MantleClient {
    pub fn new(region: String, auth: MantleAuth, base_url_override: Option<String>) -> Self {
        Self {
            http: http_client(),
            template_vars: Vars::new().with("AWS_REGION", region.clone()).with("AWS_DEFAULT_REGION", region),
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
            None => {
                self.template_vars.expand(base_url_template).map_err(|e| LlmError::InvalidArgument(e.to_string()))?
            }
        };

        Ok(format!("{}/responses", base.trim_end_matches('/')))
    }

    pub(crate) async fn stream(
        &self,
        model: &str,
        transport: &ModelTransport,
        context: &Context,
    ) -> Result<OpenedStream<SseData>> {
        let url = self.endpoint(transport)?;
        let body = build_wire_request(model, context, &ResponsesRequestPolicy::MANTLE)?;

        debug!(model, url, auth = %self.auth, "Sending Bedrock responses request");

        let encoded = serde_json::to_vec(&body)?;
        let headers = self.auth.headers("POST", &url, &encoded).await?;
        send(&self.http, &url, headers, body).await
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
