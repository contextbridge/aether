use super::mantle::{MantleAuth, MantleClient};
use super::{converse, invoke};
use crate::catalog::Provider;
use crate::catalog::transport::ModelTransport;
use crate::provider::{LlmResponseStream, StreamingModelProvider, validate_reasoning};
use crate::providers::response_stream::error_stream;
use crate::{Context, LlmModel, ProviderAuthMode, ProviderConnectionConfig};
use aws_config::Region;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sdk_bedrockruntime::config::{BehaviorVersion, Credentials};
use aws_sdk_bedrockruntime::{Client, Config};
use std::time::Duration;

const DEFAULT_MODEL: &str = "anthropic.claude-sonnet-4-5-20250929-v1:0";
const DEFAULT_REGION: &str = "us-east-1";

/// AWS credentials for explicit authentication with Bedrock.
#[derive(Clone)]
pub struct AwsCredentials {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

#[derive(Clone)]
pub struct BedrockProvider {
    client: Client,
    mantle: MantleClient,
    target: ModelTarget,
    idle_timeout: Duration,
}

#[derive(Clone)]
pub(crate) struct ModelTarget {
    pub(crate) model: String,
    pub(crate) identity: Option<LlmModel>,
    pub(crate) inference_profile_arn: Option<String>,
}

impl BedrockProvider {
    /// Create a provider using the default AWS credential chain
    /// (env vars, `~/.aws/credentials`, IAM roles, SSO).
    pub async fn new(connection: ProviderConnectionConfig) -> Self {
        if connection.auth_mode == ProviderAuthMode::None {
            return Self::from_config(None, region_from_env().as_deref(), connection);
        }

        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(url) = &connection.base_url {
            loader = loader.endpoint_url(url.clone());
        }

        let config = loader.load().await;
        let region = config
            .region()
            .map(ToString::to_string)
            .or_else(region_from_env)
            .unwrap_or_else(|| DEFAULT_REGION.to_string());
        let auth = mantle_auth(config.credentials_provider(), &region);
        Self::from_parts(Client::new(&config), region, auth, connection)
    }

    /// Create a provider from explicit configuration without async credential discovery.
    pub fn from_config(
        credentials: Option<AwsCredentials>,
        region: Option<&str>,
        connection: ProviderConnectionConfig,
    ) -> Self {
        let region = region.unwrap_or(DEFAULT_REGION).to_string();
        let auth = match connection.auth_mode {
            ProviderAuthMode::Default => mantle_auth(credentials.clone().map(shared_credentials), &region),
            ProviderAuthMode::None => MantleAuth::None,
        };
        let client = build_client(credentials, &region, connection.base_url.as_deref(), connection.auth_mode);
        Self::from_parts(client, region, auth, connection)
    }

    fn from_parts(client: Client, region: String, auth: MantleAuth, connection: ProviderConnectionConfig) -> Self {
        let mantle = MantleClient::new(region, auth, connection.base_url.clone());
        Self {
            client,
            mantle,
            target: ModelTarget::new(DEFAULT_MODEL, connection.inference_profile_arn),
            idle_timeout: connection.idle_timeout,
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.target = ModelTarget::new(model, self.target.inference_profile_arn.take());
        self
    }

    pub fn with_inference_profile_arn(mut self, arn: impl Into<String>) -> Self {
        self.target.inference_profile_arn = Some(arn.into());
        self
    }

    /// Authenticate Responses-transport requests with a Bedrock API key instead
    /// of `SigV4`. Equivalent to setting `AWS_BEARER_TOKEN_BEDROCK`.
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.mantle = self.mantle.with_auth(MantleAuth::BearerToken(token.into()));
        self
    }
}

impl ModelTarget {
    fn new(model: &str, inference_profile_arn: Option<String>) -> Self {
        let identity = LlmModel::parse_for(Provider::Bedrock, model).ok();
        Self { model: model.to_string(), identity, inference_profile_arn }
    }

    pub(crate) fn request_id(&self) -> &str {
        self.inference_profile_arn.as_deref().unwrap_or(&self.model)
    }
}

impl StreamingModelProvider for BedrockProvider {
    fn model(&self) -> Option<LlmModel> {
        self.target.identity.clone()
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        if let Err(error) = validate_reasoning(context, self.target.identity.as_ref()) {
            return error_stream(error);
        }
        let (target, context) = (self.target.clone(), context.clone());

        match target.identity.as_ref().and_then(LlmModel::transport) {
            None => converse::stream(&self.client, target, context, self.idle_timeout),
            Some(ModelTransport::AnthropicMessages) => invoke::stream(&self.client, target, context, self.idle_timeout),
            Some(ModelTransport::OpenAiResponses { base_url_template }) => {
                self.mantle.stream(target, base_url_template, context, self.idle_timeout)
            }
        }
    }
}

fn build_client(
    credentials: Option<AwsCredentials>,
    region: &str,
    base_url: Option<&str>,
    auth_mode: ProviderAuthMode,
) -> Client {
    let mut config =
        Config::builder().behavior_version(BehaviorVersion::latest()).region(Region::new(region.to_string()));

    if auth_mode == ProviderAuthMode::None {
        config = config.allow_no_auth();
    } else if let Some(credentials) = credentials {
        config = config.credentials_provider(shared_credentials(credentials));
    }
    if let Some(url) = base_url {
        config = config.endpoint_url(url);
    }

    Client::from_conf(config.build())
}

fn shared_credentials(credentials: AwsCredentials) -> SharedCredentialsProvider {
    SharedCredentialsProvider::new(Credentials::new(
        credentials.access_key_id,
        credentials.secret_access_key,
        credentials.session_token,
        None,
        "aether-bedrock-provider",
    ))
}

/// Resolve the credential scheme for the Responses transport.
///
/// A Bedrock API key takes precedence over the credential chain because it is
/// the scheme the model catalog advertises for these endpoints; `SigV4` keeps
/// SSO and IAM-role users working without extra configuration.
fn mantle_auth(credentials: Option<SharedCredentialsProvider>, region: &str) -> MantleAuth {
    if let Some(token) = MantleAuth::bearer_token_from_env() {
        return MantleAuth::BearerToken(token);
    }
    match credentials {
        Some(credentials) => MantleAuth::sigv4(credentials, region),
        None => MantleAuth::None,
    }
}

fn region_from_env() -> Option<String> {
    ["AWS_REGION", "AWS_DEFAULT_REGION"].into_iter().find_map(|name| match std::env::var(name) {
        Ok(value) if !value.is_empty() => Some(value),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::anthropic::test_stream::{AnthropicStreamBuilder, anthropic_stream, usage};
    use crate::providers::test_capture_server::{CaptureServer, ResponseSpec, hello_context};
    use crate::reasoning::ReasoningControl;
    use crate::testing::{assistant_message, llm_response, tool_result};
    use crate::{ChatMessage, LlmError, LlmResponse, StopReason};
    use aws_smithy_eventstream::frame::write_message_to;
    use aws_smithy_types::base64;
    use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
    use futures::StreamExt;
    use serde_json::json;
    use utils::ReasoningEffort;

    #[test]
    fn test_display_name() {
        assert_eq!(test_provider().display_name(), "AWS Bedrock (anthropic.claude-sonnet-4-5-20250929-v1:0)");
    }

    #[test]
    fn test_with_model() {
        let provider = test_provider().with_model("anthropic.claude-opus-4-20250514-v1:0");
        assert_eq!(provider.display_name(), "AWS Bedrock (anthropic.claude-opus-4-20250514-v1:0)");
    }

    #[test]
    fn test_default_values() {
        let provider = test_provider();
        assert_eq!(provider.target.model, "anthropic.claude-sonnet-4-5-20250929-v1:0");
    }

    #[tokio::test]
    async fn auth_none_sends_unsigned_request_to_custom_endpoint() {
        let mut server = CaptureServer::start_with_response("{}").await;
        let provider = create_provider(&server.base_url, DEFAULT_MODEL).await;

        let _ = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let request = server.captured().await;

        assert!(request.path.starts_with("/model/"), "{}", request.path);
        assert!(!request.headers.contains_key("authorization"), "request was signed: {:?}", request.headers);
        assert!(
            !request.headers.contains_key("x-amz-security-token"),
            "request included session token: {:?}",
            request.headers
        );
    }

    fn static_credentials() -> AwsCredentials {
        AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        }
    }

    #[tokio::test]
    async fn mantle_disabled_uses_none_without_summary() {
        let model = responses_models().find(|model| model.supports_reasoning_off()).unwrap();
        let mut server = CaptureServer::start_responses().await;
        let provider = create_provider(&server.base_url, &model.model_id()).await;
        let mut context = hello_context();
        context.set_reasoning_effort(ReasoningEffort::Disabled);
        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        let body = server.captured().await.body;
        assert_eq!(body["reasoning"]["effort"], "none");
        assert!(body["reasoning"]["summary"].is_null());
    }

    #[tokio::test]
    async fn converse_disabled_is_a_non_retryable_transport_error() {
        let model = LlmModel::all()
            .iter()
            .find(|model| {
                model.provider_enum() == Provider::Bedrock
                    && model.transport().is_none()
                    && model.supports_reasoning_off()
            })
            .unwrap();
        let provider =
            BedrockProvider::new(ProviderConnectionConfig { auth_mode: ProviderAuthMode::None, ..Default::default() })
                .await
                .with_model(&model.model_id());
        let mut context = hello_context();
        context.set_reasoning_effort(crate::ReasoningEffort::Disabled);
        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert_eq!(responses.len(), 1);
        assert!(matches!(&responses[0], Err(LlmError::UnsupportedDisableTransport { .. })));
        assert!(!responses[0].as_ref().unwrap_err().is_retryable());
    }

    #[tokio::test]
    async fn responses_shape_models_are_sent_to_the_responses_endpoint() {
        let mut server = CaptureServer::start_responses().await;
        let provider = mantle_provider(&server).await;
        let mut context = hello_context();
        context.set_reasoning_effort(crate::ReasoningEffort::Xhigh);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(!responses.is_empty());
        assert_eq!(captured.path, "/responses");
        assert_eq!(captured.body["model"], mantle_model());
        assert_eq!(captured.body["stream"], true);
        assert_eq!(captured.body["store"], false);
        assert_eq!(captured.body["reasoning"]["effort"], "xhigh");
        assert_eq!(captured.body["input"][0]["role"], "user");
        assert_eq!(captured.body["input"][0]["content"][0]["type"], "input_text");
        assert_eq!(captured.body["input"][0]["content"][0]["text"], "Hello");
        assert!(captured.headers.get("authorization").is_none(), "{:?}", captured.headers);
    }

    #[tokio::test]
    async fn http_200_failed_server_error_is_retryable_with_diagnostics() {
        let spec = ResponseSpec::sse(include_str!("../../../tests/fixtures/openai_responses/04_failed_server.sse"))
            .with_header("x-amzn-requestid", "amzn-req-123");
        let mut server = CaptureServer::start_with_spec(spec).await;
        let provider = mantle_provider(&server).await;

        let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let _ = server.captured().await;

        assert!(!responses.iter().any(|r| matches!(r, Ok(crate::LlmResponse::Done { .. }))));
        let err = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert!(err.is_retryable(), "server_error must be retryable: {err:?}");
        let provider_error = err.provider().expect("expected provider error");
        assert_eq!(provider_error.kind, crate::ProviderErrorKind::Server);
        assert_eq!(provider_error.code.as_deref(), Some("server_error"));
        assert_eq!(provider_error.http_status, Some(200));
        assert_eq!(provider_error.request_id.as_deref(), Some("amzn-req-123"));
    }

    #[tokio::test]
    async fn failed_event_with_unknown_code_is_terminal_without_done() {
        let body = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\nevent: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"invalid_prompt\",\"message\":\"bad prompt\"}}}\n\n";
        let mut server = CaptureServer::start_with_response(body).await;
        let provider = mantle_provider(&server).await;

        let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let _ = server.captured().await;

        assert!(!responses.iter().any(|r| matches!(r, Ok(crate::LlmResponse::Done { .. }))));
        let err = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert!(!err.is_retryable(), "invalid_prompt must be terminal: {err:?}");
    }

    #[tokio::test]
    async fn responses_transport_rejects_malformed_and_truncated_streams() {
        for response in [
            "data: {not-json}\n\n",
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\"}}\n\ndata: [DONE]\n\n",
        ] {
            let mut server = CaptureServer::start_with_response(response).await;
            let provider = mantle_provider(&server).await;

            let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
            let _ = server.captured().await;

            assert!(responses.iter().any(|response| {
                response.as_ref().err().and_then(LlmError::provider).map(|provider| provider.kind)
                    == Some(crate::ProviderErrorKind::StreamInterrupted)
            }));
            assert!(!responses.iter().any(|response| matches!(response, Ok(crate::LlmResponse::Done { .. }))));
        }
    }

    #[tokio::test]
    async fn responses_transport_drops_encrypted_reasoning_from_another_model() {
        let mut server = CaptureServer::start_responses().await;
        let provider = mantle_provider(&server).await;
        let context = Context::new(
            vec![
                assistant_message()
                    .text("previous answer")
                    .encrypted_reasoning("reasoning-id", "bedrock:openai.gpt-5.5", "opaque-for-another-model")
                    .build(),
            ],
            vec![],
        );

        let _ = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(
            captured.body["input"].as_array().unwrap().iter().all(|item| item["type"] != "reasoning"),
            "{}",
            captured.body
        );
    }

    #[tokio::test]
    async fn claude_models_send_anthropic_messages_through_invoke_model() {
        let mut server = CaptureServer::start_with_spec(invoke_response(claude_answer())).await;
        let model = claude_model(|model| {
            model.reasoning_control() == ReasoningControl::Effort
                && model.validate_reasoning_effort(ReasoningEffort::High).is_ok()
        });
        let provider = create_provider(&server.base_url, &model.model_id()).await;
        let mut context = Context::new(vec![ChatMessage::system("Be brief"), ChatMessage::user("Hello")], vec![]);
        context.set_reasoning_effort(ReasoningEffort::High);

        let responses = streamed(&provider, &context).await;
        let request = server.captured().await;

        assert_eq!(responses, llm_response().text(&["Hi"]).usage(10, 5).build_with_stop_reason(StopReason::EndTurn));
        assert!(request.path.ends_with("/invoke-with-response-stream"), "{}", request.path);
        assert_eq!(
            request.body,
            json!({
                "anthropic_version": "bedrock-2023-05-31",
                "max_tokens": 16_384,
                "system": [{ "type": "text", "text": "Be brief", "cache_control": { "type": "ephemeral" } }],
                "messages": [{
                    "role": "user",
                    "content": [{ "type": "text", "text": "Hello", "cache_control": { "type": "ephemeral" } }]
                }],
                "thinking": { "type": "adaptive", "display": "summarized" },
                "output_config": { "effort": "high" }
            })
        );
    }

    #[tokio::test]
    async fn claude_models_turn_thinking_off_when_the_catalog_allows_it() {
        let mut server = CaptureServer::start_with_spec(invoke_response(claude_answer())).await;
        let model = claude_model(|model| model.validate_reasoning_effort(ReasoningEffort::Disabled).is_ok());
        let provider = create_provider(&server.base_url, &model.model_id()).await;
        let mut context = hello_context();
        context.set_reasoning_effort(ReasoningEffort::Disabled);

        let responses = streamed(&provider, &context).await;
        let request = server.captured().await;

        assert_eq!(responses, llm_response().text(&["Hi"]).usage(10, 5).build_with_stop_reason(StopReason::EndTurn));
        assert_eq!(request.body["thinking"], json!({ "type": "disabled" }));
    }

    #[tokio::test]
    async fn claude_models_stream_reasoning_usage_and_completion_through_invoke_model() {
        let server = CaptureServer::start_with_spec(invoke_response(claude_tool_turn())).await;
        let provider = create_provider(&server.base_url, DEFAULT_MODEL).await;

        let responses = streamed(&provider, &hello_context()).await;

        assert_eq!(
            responses,
            llm_response()
                .reasoning(&["Plan"])
                .tool_call("toolu_1", "search", &["{}"])
                .usage(10, 30)
                .encrypted_reasoning("msg_123", r#"[{"type":"thinking","thinking":"Plan","signature":"sig"}]"#)
                .build_with_stop_reason(StopReason::ToolCalls)
        );
    }

    #[tokio::test]
    async fn claude_reasoning_from_invoke_model_is_replayed_on_the_next_turn() {
        let mut server = CaptureServer::start_with_spec(invoke_response(claude_tool_turn())).await;
        let provider = create_provider(&server.base_url, DEFAULT_MODEL).await;
        let first_turn = streamed(&provider, &hello_context()).await;
        server.captured().await;
        let Some(LlmResponse::EncryptedReasoning { id, content }) =
            first_turn.iter().find(|response| matches!(response, LlmResponse::EncryptedReasoning { .. }))
        else {
            panic!("the first turn must return replayable reasoning: {first_turn:?}");
        };

        let context = Context::new(
            vec![
                ChatMessage::user("Hello"),
                assistant_message()
                    .encrypted_reasoning(id, &format!("bedrock:{DEFAULT_MODEL}"), content)
                    .tool_call("toolu_1", "search", "{}")
                    .build(),
                tool_result("toolu_1", "search", "{}", "found"),
            ],
            vec![],
        );
        streamed(&provider, &context).await;
        let request = server.captured().await;

        assert_eq!(
            request.body["messages"][1]["content"],
            json!([
                { "type": "thinking", "thinking": "Plan", "signature": "sig" },
                { "type": "tool_use", "id": "toolu_1", "name": "search", "input": {} }
            ])
        );
    }

    #[tokio::test]
    async fn non_claude_models_still_use_converse() {
        let mut server = CaptureServer::start_with_response("{}").await;
        let model = LlmModel::all()
            .iter()
            .find(|model| model.provider_enum() == Provider::Bedrock && model.transport().is_none())
            .expect("the catalog has a Bedrock model served by Converse");
        let provider = create_provider(&server.base_url, &model.model_id()).await;

        let _ = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let request = server.captured().await;

        assert!(request.path.ends_with("/converse-stream"), "{}", request.path);
    }

    #[tokio::test]
    async fn bearer_token_authenticates_responses_requests() {
        let mut server = CaptureServer::start_responses().await;
        let provider = BedrockProvider::from_config(
            None,
            Some("us-west-2"),
            ProviderConnectionConfig { base_url: Some(server.base_url.clone()), ..Default::default() },
        )
        .with_bearer_token("test-token")
        .with_model(&mantle_model());

        let _ = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert_eq!(captured.headers.get("authorization").unwrap(), "Bearer test-token");
    }

    #[tokio::test]
    async fn credential_chain_sigv4_signs_responses_requests() {
        let mut server = CaptureServer::start_responses().await;
        let provider = BedrockProvider::from_config(
            Some(static_credentials()),
            Some("us-west-2"),
            ProviderConnectionConfig { base_url: Some(server.base_url.clone()), ..Default::default() },
        )
        .with_model(&mantle_model());

        let _ = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        let authorization = captured.headers.get("authorization").expect("request was not signed").to_str().unwrap();
        assert!(
            authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/"),
            "unexpected authorization header: {authorization}"
        );
        assert!(authorization.contains("/us-west-2/bedrock/aws4_request"), "{authorization}");
        assert!(captured.headers.contains_key("x-amz-date"), "{:?}", captured.headers);
    }

    #[test]
    fn explicit_connection_preserves_inference_profile() {
        let provider = BedrockProvider::from_config(
            None,
            Some("us-west-2"),
            ProviderConnectionConfig { inference_profile_arn: Some("arn:test".to_string()), ..Default::default() },
        );

        assert_eq!(provider.target.inference_profile_arn.as_deref(), Some("arn:test"));
    }

    #[test]
    fn test_from_config_with_credentials() {
        let provider =
            BedrockProvider::from_config(Some(static_credentials()), None, ProviderConnectionConfig::default());
        assert_eq!(provider.target.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_from_config_with_credentials_and_region() {
        let credentials =
            AwsCredentials { session_token: Some("FwoGZXIvYXdzEBYaD...".to_string()), ..static_credentials() };

        let provider =
            BedrockProvider::from_config(Some(credentials), Some("us-west-2"), ProviderConnectionConfig::default())
                .with_model("anthropic.claude-opus-4-20250514-v1:0");

        assert_eq!(provider.target.model, "anthropic.claude-opus-4-20250514-v1:0");
    }

    #[test]
    fn test_from_config_with_region_only() {
        let provider = BedrockProvider::from_config(None, Some("eu-west-1"), ProviderConnectionConfig::default());
        assert_eq!(provider.target.model, DEFAULT_MODEL);
    }

    #[test]
    fn catalog_foundation_id_resolves_context_window() {
        let provider = test_provider().with_model("anthropic.claude-sonnet-4-5-20250929-v1:0");
        assert!(provider.context_window().is_some());
        assert_eq!(provider.model().unwrap().to_string(), "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0");
    }

    #[test]
    fn cross_region_profile_id_in_catalog_resolves() {
        let provider = test_provider().with_model("us.anthropic.claude-opus-4-6-v1");
        assert!(provider.context_window().is_some());
    }

    #[test]
    fn unknown_cross_region_profile_id_falls_through_to_profile() {
        let id = "us.anthropic.claude-future-model-v99:0";
        let provider = test_provider().with_model(id);
        assert_eq!(provider.context_window(), None);
        assert_eq!(provider.model().unwrap().to_string(), format!("bedrock:{id}"));
        assert_eq!(provider.display_name(), format!("AWS Bedrock ({id})"));
    }

    #[tokio::test]
    async fn separate_inference_profile_arn_is_used_as_request_model_id() {
        let mut server = CaptureServer::start_with_response("{}").await;
        let provider = BedrockProvider::new(ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            inference_profile_arn: Some(application_inference_profile_arn().to_string()),
            ..Default::default()
        })
        .await
        .with_model(DEFAULT_MODEL);

        let _ = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let request = server.captured().await;

        assert!(
            request.path.contains("arn%3Aaws%3Abedrock%3Aus-west-2%3A000000000000%3Aapplication-inference-profile"),
            "{}",
            request.path
        );
        assert_eq!(provider.context_window(), Some(200_000));
        assert_eq!(provider.model().unwrap().to_string(), "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0");
    }

    #[test]
    fn with_inference_profile_arn_keeps_canonical_model_identity() {
        let arn = inference_profile_arn("us.anthropic.claude-sonnet-4-5-20250929-v1:0");
        let provider =
            test_provider().with_model("anthropic.claude-sonnet-4-5-20250929-v1:0").with_inference_profile_arn(&arn);

        assert_eq!(provider.target.request_id(), arn);
        assert_eq!(provider.context_window(), Some(200_000));
        assert_eq!(provider.model().unwrap().to_string(), "bedrock:anthropic.claude-sonnet-4-5-20250929-v1:0");
    }

    #[test]
    fn prompt_caching_support_comes_from_canonical_model() {
        let cached = test_provider().with_model("anthropic.claude-sonnet-4-5-20250929-v1:0");
        assert!(cached.model().unwrap().supports_prompt_caching());

        let unknown_profile = test_provider().with_model("us.anthropic.claude-future-model-v99:0");
        assert!(!unknown_profile.model().unwrap().supports_prompt_caching());
    }

    fn inference_profile_arn(model: &str) -> String {
        format!("arn:aws:bedrock:us-west-2:000000000000:inference-profile/{model}")
    }

    fn application_inference_profile_arn() -> &'static str {
        "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000"
    }

    fn test_provider() -> BedrockProvider {
        BedrockProvider::from_config(None, None, ProviderConnectionConfig::default())
    }

    fn mantle_model() -> String {
        responses_models()
            .next()
            .expect("catalog must expose at least one Responses-transport Bedrock model")
            .model_id()
            .to_string()
    }

    fn responses_models() -> impl Iterator<Item = &'static LlmModel> {
        LlmModel::all().iter().filter(|model| {
            model.provider_enum() == Provider::Bedrock
                && matches!(model.transport(), Some(ModelTransport::OpenAiResponses { .. }))
        })
    }

    async fn mantle_provider(server: &CaptureServer) -> BedrockProvider {
        create_provider(&server.base_url, &mantle_model()).await
    }

    fn claude_model(matches: impl Fn(&LlmModel) -> bool) -> &'static LlmModel {
        LlmModel::all()
            .iter()
            .find(|model| model.transport() == Some(ModelTransport::AnthropicMessages) && matches(model))
            .expect("the catalog has a matching Bedrock Claude model")
    }

    async fn streamed(provider: &BedrockProvider, context: &Context) -> Vec<LlmResponse> {
        provider.stream_response(context).map(|response| response.unwrap()).collect().await
    }

    fn claude_answer() -> AnthropicStreamBuilder {
        anthropic_stream().text(0, &["Hi"]).message_delta("end_turn", &usage(10, 5)).message_stop()
    }

    /// Bedrock reports input tokens only in `message_start`.
    fn claude_tool_turn() -> AnthropicStreamBuilder {
        anthropic_stream()
            .signed_thinking(0, &["Plan"], "sig")
            .tool_call(1, "toolu_1", "search", &["{}"])
            .message_delta("tool_use", &json!({ "output_tokens": 30 }))
            .message_stop()
    }

    fn invoke_response(stream: AnthropicStreamBuilder) -> ResponseSpec {
        let mut body = Vec::new();
        for event in stream.build() {
            let message = Message::new(json!({ "bytes": base64::encode(event) }).to_string())
                .add_header(Header::new(":message-type", HeaderValue::String("event".into())))
                .add_header(Header::new(":event-type", HeaderValue::String("chunk".into())))
                .add_header(Header::new(":content-type", HeaderValue::String("application/json".into())));
            write_message_to(&message, &mut body).unwrap();
        }
        ResponseSpec::event_stream(body)
    }

    async fn create_provider(base_url: &str, model: &str) -> BedrockProvider {
        BedrockProvider::new(ProviderConnectionConfig {
            base_url: Some(base_url.to_string()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        })
        .await
        .with_model(model)
    }
}
