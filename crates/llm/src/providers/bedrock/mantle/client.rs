use std::time::Duration;
use tracing::{debug, warn};

use super::auth::MantleAuth;
use crate::providers::bedrock::provider::ModelTarget;
use crate::providers::http::{SseData, http_client, post_sse, responses_code};
use crate::providers::openai::responses::mappers::{ResponsesRequestPolicy, build_request};
use crate::providers::openai::responses::streaming::decode_responses;
use crate::providers::response_stream::{OpenedStream, response_stream};
use crate::{Context, LlmError, LlmResponseStream, Result};
use utils::variables::Vars;

/// Transport for Bedrock models served through the `OpenAI` Responses API.
#[derive(Clone)]
pub(crate) struct MantleClient {
    http: reqwest::Client,
    template_vars: Vars,
    auth: MantleAuth,
    base_url_override: Option<String>,
}

impl MantleClient {
    pub(crate) fn new(region: String, auth: MantleAuth, base_url_override: Option<String>) -> Self {
        Self {
            http: http_client(),
            template_vars: Vars::new().with("AWS_REGION", region.clone()).with("AWS_DEFAULT_REGION", region),
            auth,
            base_url_override: base_url_override.map(|url| url.trim_end_matches('/').to_string()),
        }
    }

    pub(crate) fn with_auth(mut self, auth: MantleAuth) -> Self {
        self.auth = auth;
        self
    }

    pub(crate) fn stream(
        &self,
        target: ModelTarget,
        base_url_template: &'static str,
        context: Context,
        idle_timeout: Duration,
    ) -> LlmResponseStream {
        if let Some(arn) = &target.inference_profile_arn {
            warn!(
                model = %target.model,
                inference_profile_arn = %arn,
                "Ignoring inferenceProfileArn: Responses requests use the selected model ID"
            );
        }
        let client = self.clone();

        response_stream(
            async move { client.send(&target, base_url_template, &context).await },
            decode_responses(),
            idle_timeout,
        )
    }

    /// An explicit base URL — a test server or a private endpoint — wins over the
    /// catalog template, which is why the override is checked first.
    pub(crate) fn endpoint(&self, base_url_template: &str) -> Result<String> {
        let base = match &self.base_url_override {
            Some(base) => base.clone(),
            None => {
                self.template_vars.expand(base_url_template).map_err(|e| LlmError::InvalidArgument(e.to_string()))?
            }
        };

        Ok(format!("{}/responses", base.trim_end_matches('/')))
    }

    async fn send(
        &self,
        target: &ModelTarget,
        base_url_template: &str,
        context: &Context,
    ) -> Result<OpenedStream<SseData>> {
        let url = self.endpoint(base_url_template)?;
        let model = target.model.as_str();
        let body = build_request(target.identity.as_ref(), model, context, &ResponsesRequestPolicy::MANTLE)?;

        debug!(model, url, auth = %self.auth, "Sending Bedrock responses request");

        let encoded = serde_json::to_vec(&body)?;
        let headers = self.auth.headers("POST", &url, &encoded).await?;
        post_sse(&self.http, &url, headers, &body, responses_code).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_expands_the_region_from_the_model_template() {
        let endpoint = client(None).endpoint("https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1").unwrap();

        assert_eq!(endpoint, "https://bedrock-mantle.us-west-2.api.aws/openai/v1/responses");
    }

    #[test]
    fn endpoint_preserves_a_models_own_api_path() {
        let endpoint = client(None).endpoint("https://bedrock-mantle.${AWS_REGION}.api.aws/v1").unwrap();

        assert_eq!(endpoint, "https://bedrock-mantle.us-west-2.api.aws/v1/responses");
    }

    #[test]
    fn base_url_override_replaces_the_catalog_template() {
        let endpoint = client(Some("http://127.0.0.1:9999/".to_string()))
            .endpoint("https://bedrock-mantle.${AWS_REGION}.api.aws/openai/v1")
            .unwrap();

        assert_eq!(endpoint, "http://127.0.0.1:9999/responses");
    }

    fn client(base_url_override: Option<String>) -> MantleClient {
        MantleClient::new("us-west-2".to_string(), MantleAuth::None, base_url_override)
    }
}
