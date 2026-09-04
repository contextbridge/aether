use super::mantle::{MantleAuth, MantleClient};
use super::mappers::{default_cache_point, map_messages, map_tools};
use super::streaming::process_bedrock_stream;
use crate::catalog::transport::ModelTransport;
use crate::provider::{LlmResponseStream, ProviderFactory, StreamingModelProvider, get_context_window, stream_from};
use crate::providers::openai_responses::transport::process_connection;
use crate::{Context, LlmError, ProviderAuthMode, ProviderConnectionConfig, ProviderError, Result};
use aws_config::Region;
use aws_credential_types::provider::SharedCredentialsProvider;
use aws_sdk_bedrockruntime::config::{BehaviorVersion, Credentials};
use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError;
use aws_sdk_bedrockruntime::types::{ConverseStreamOutput, InferenceConfiguration};
use aws_sdk_bedrockruntime::{Client, Config};
use tracing::{error, info, warn};

const DEFAULT_MODEL: &str = "anthropic.claude-sonnet-4-5-20250929-v1:0";
const DEFAULT_MAX_TOKENS: i32 = 16_384;
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
    model: String,
    inference_profile_arn: Option<String>,
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
        Self::assemble(Client::new(&config), region, auth, connection)
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
        Self::assemble(client, region, auth, connection)
    }

    fn assemble(client: Client, region: String, auth: MantleAuth, connection: ProviderConnectionConfig) -> Self {
        let mantle = MantleClient::new(region, auth, connection.base_url.clone());
        Self {
            client,
            mantle,
            model: DEFAULT_MODEL.to_string(),
            inference_profile_arn: connection.inference_profile_arn,
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    pub fn with_inference_profile_arn(mut self, arn: impl Into<String>) -> Self {
        self.inference_profile_arn = Some(arn.into());
        self
    }

    /// Authenticate Responses-transport requests with a Bedrock API key instead
    /// of `SigV4`. Equivalent to setting `AWS_BEARER_TOKEN_BEDROCK`.
    pub fn with_bearer_token(mut self, token: impl Into<String>) -> Self {
        self.mantle = self.mantle.with_auth(MantleAuth::BearerToken(token.into()));
        self
    }

    fn request_model_id(&self) -> &str {
        self.inference_profile_arn.as_deref().unwrap_or(&self.model)
    }

    /// The Responses-API transport for the current model, when the catalog says
    /// it is not served by the Converse API.
    fn mantle_transport(&self) -> Option<ModelTransport> {
        self.model().and_then(|model| model.transport())
    }

    async fn send_converse_stream(
        &self,
        context: &Context,
    ) -> Result<EventReceiver<ConverseStreamOutput, ConverseStreamOutputError>> {
        let cache_point =
            self.model().is_some_and(|m| m.supports_prompt_caching()).then(default_cache_point).transpose()?;
        let (system_blocks, messages) = map_messages(context.messages(), cache_point.as_ref())?;
        let settings = context.model_settings();
        let max_tokens = settings.max_tokens.and_then(|m| i32::try_from(m).ok()).unwrap_or(DEFAULT_MAX_TOKENS);
        let mut inference_config = InferenceConfiguration::builder().max_tokens(max_tokens);

        if let Some(temp) = settings.temperature {
            inference_config = inference_config.temperature(temp);
        }

        if let Some(top_p) = settings.top_p {
            inference_config = inference_config.top_p(top_p);
        }

        let inference_config = inference_config.build();

        let mut request = self
            .client
            .converse_stream()
            .model_id(self.request_model_id())
            .set_messages(Some(messages))
            .inference_config(inference_config);

        if !system_blocks.is_empty() {
            request = request.set_system(Some(system_blocks));
        }

        if !context.tools().is_empty() {
            let tool_config = map_tools(context.tools(), cache_point.as_ref())?;
            request = request.tool_config(tool_config);
        }

        if let Some(arn) = self.inference_profile_arn.as_deref() {
            info!(model = %self.model, inference_profile_arn = %arn, "Sending Bedrock converse_stream request");
        } else {
            info!(model = %self.model, "Sending Bedrock converse_stream request");
        }

        let response = request.send().await.map_err(|e| {
            error!(model = %self.model, error = ?e, "Bedrock API error");
            LlmError::from(e)
        })?;

        Ok(response.stream)
    }
}

impl ProviderFactory for BedrockProvider {
    async fn from_env() -> Result<Self> {
        Ok(Self::new(ProviderConnectionConfig::default()).await)
    }

    async fn from_env_with_connection(connection: ProviderConnectionConfig) -> Result<Self> {
        Ok(Self::new(connection).await)
    }

    fn with_model(self, model: &str) -> Self {
        self.with_model(model)
    }
}

impl StreamingModelProvider for BedrockProvider {
    fn model(&self) -> Option<crate::LlmModel> {
        format!("bedrock:{}", self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window("bedrock", &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let provider = self.clone();
        let context = context.clone();

        let Some(transport) = self.mantle_transport() else {
            return stream_from(async move { provider.send_converse_stream(&context).await }, process_bedrock_stream);
        };

        if let Some(arn) = self.inference_profile_arn.as_deref() {
            warn!(
                model = %self.model,
                inference_profile_arn = %arn,
                "Ignoring inferenceProfileArn: this model is served by the Responses API, which has no inference profiles"
            );
        }

        stream_from(
            async move { provider.mantle.stream(&provider.model, &transport, &context).await },
            process_connection,
        )
    }

    fn display_name(&self) -> String {
        format!("Bedrock ({})", self.model)
    }
}

impl From<SdkError<ConverseStreamError>> for LlmError {
    fn from(e: SdkError<ConverseStreamError>) -> Self {
        let message = format!("Bedrock API error: {e}");
        let status = e.raw_response().map(|r| r.status().as_u16());
        let request_id = e.raw_response().and_then(|r| r.headers().get("x-amzn-requestid")).map(str::to_string);
        let mut provider = match &e {
            SdkError::TimeoutError(_) => ProviderError::timeout(message),
            SdkError::DispatchFailure(_) => ProviderError::network(message),
            SdkError::ResponseError(_) => ProviderError::server(message),
            SdkError::ServiceError(svc) => {
                let inner = svc.err();
                if inner.is_throttling_exception() {
                    ProviderError::rate_limit(message)
                } else if inner.is_service_unavailable_exception()
                    || inner.is_internal_server_exception()
                    || inner.is_model_stream_error_exception()
                {
                    ProviderError::server(message)
                } else {
                    ProviderError::api(message)
                }
            }
            _ => ProviderError::api(message),
        };
        provider = provider.with_http_metadata(status, request_id);
        Self::from(provider)
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
        Some(credentials) => MantleAuth::SigV4 { credentials, region: region.to_string() },
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
    use crate::catalog::Provider;
    use crate::providers::test_capture_server::CaptureServer;
    use crate::types::IsoString;
    use crate::{AssistantReasoning, ChatMessage, EncryptedReasoningContent, LlmModel};
    use axum::Router;
    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap, Method, Request, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use futures::StreamExt;
    use std::sync::Arc;
    use tokio::net::TcpListener;
    use tokio::sync::{Mutex, oneshot};

    fn inference_profile_arn(model: &str) -> String {
        format!("arn:aws:bedrock:us-west-2:000000000000:inference-profile/{model}")
    }

    fn application_inference_profile_arn() -> &'static str {
        "arn:aws:bedrock:us-west-2:000000000000:application-inference-profile/000000000000"
    }

    fn test_provider() -> BedrockProvider {
        BedrockProvider::from_config(None, None, ProviderConnectionConfig::default())
    }

    /// A catalog model routed to the Responses transport, resolved from the
    /// catalog so a models.dev sync that retires one model does not quietly
    /// leave these tests exercising the Converse path instead.
    fn mantle_model() -> String {
        LlmModel::all()
            .iter()
            .find(|model| model.provider_enum() == Provider::Bedrock && model.transport().is_some())
            .expect("catalog must expose at least one Responses-transport Bedrock model")
            .model_id()
            .to_string()
    }

    /// A provider talking to `server` over the Responses transport, unauthenticated.
    async fn mantle_provider(server: &CaptureServer) -> BedrockProvider {
        BedrockProvider::new(ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        })
        .await
        .with_model(&mantle_model())
    }

    #[test]
    fn test_display_name() {
        assert_eq!(test_provider().display_name(), "Bedrock (anthropic.claude-sonnet-4-5-20250929-v1:0)");
    }

    #[test]
    fn test_with_model() {
        let provider = test_provider().with_model("anthropic.claude-opus-4-20250514-v1:0");
        assert_eq!(provider.display_name(), "Bedrock (anthropic.claude-opus-4-20250514-v1:0)");
    }

    #[test]
    fn test_default_values() {
        let provider = test_provider();
        assert_eq!(provider.model, "anthropic.claude-sonnet-4-5-20250929-v1:0");
    }

    #[tokio::test]
    async fn auth_none_sends_unsigned_request_to_custom_endpoint() {
        let endpoint = FakeBedrockEndpoint::start().await;
        let provider = BedrockProvider::new(ProviderConnectionConfig {
            base_url: Some(endpoint.url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        })
        .await;

        let result = provider.send_converse_stream(&hello_context()).await;
        let request = endpoint.request.await.expect("fake Bedrock endpoint received no request");

        assert!(result.is_err());
        assert_eq!(request.method, Method::POST);
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

    fn hello_context() -> Context {
        Context::new(vec![ChatMessage::user("Hello")], vec![])
    }

    #[tokio::test]
    async fn responses_shape_models_are_sent_to_the_responses_endpoint() {
        let mut server = CaptureServer::start_responses().await;
        let provider = mantle_provider(&server).await;
        let mut context = hello_context();
        context.set_reasoning_effort(Some(crate::ReasoningEffort::Xhigh));

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
        use crate::providers::test_capture_server::ResponseSpec;
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
            vec![ChatMessage::Assistant {
                content: "previous answer".to_string(),
                reasoning: AssistantReasoning {
                    summary_text: None,
                    encrypted_content: Some(EncryptedReasoningContent {
                        id: "reasoning-id".to_string(),
                        model: "bedrock:openai.gpt-5.5".parse().unwrap(),
                        content: "opaque-for-another-model".to_string(),
                    }),
                },
                timestamp: IsoString::now(),
                tool_calls: vec![],
            }],
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
    async fn converse_shape_models_do_not_use_the_responses_endpoint() {
        let endpoint = FakeBedrockEndpoint::start().await;
        let provider = BedrockProvider::new(ProviderConnectionConfig {
            base_url: Some(endpoint.url.clone()),
            auth_mode: ProviderAuthMode::None,
            ..Default::default()
        })
        .await
        .with_model(DEFAULT_MODEL);

        let _ = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let request = endpoint.request.await.expect("fake Bedrock endpoint received no request");

        assert!(request.path.starts_with("/model/"), "{}", request.path);
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
    fn only_responses_shape_models_route_to_the_mantle_transport() {
        let mantle = test_provider().with_model(&mantle_model());
        let converse = test_provider().with_model(DEFAULT_MODEL);
        let profile = test_provider().with_model("us.anthropic.claude-future-model-v99:0");

        assert!(matches!(mantle.mantle_transport(), Some(ModelTransport::OpenAiResponses { .. })));
        assert_eq!(converse.mantle_transport(), None);
        assert_eq!(profile.mantle_transport(), None);
    }

    #[test]
    fn explicit_connection_preserves_inference_profile() {
        let provider = BedrockProvider::from_config(
            None,
            Some("us-west-2"),
            ProviderConnectionConfig { inference_profile_arn: Some("arn:test".to_string()), ..Default::default() },
        );

        assert_eq!(provider.inference_profile_arn.as_deref(), Some("arn:test"));
    }

    #[test]
    fn test_from_config_with_credentials() {
        let provider =
            BedrockProvider::from_config(Some(static_credentials()), None, ProviderConnectionConfig::default());
        assert_eq!(provider.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_from_config_with_credentials_and_region() {
        let credentials =
            AwsCredentials { session_token: Some("FwoGZXIvYXdzEBYaD...".to_string()), ..static_credentials() };

        let provider =
            BedrockProvider::from_config(Some(credentials), Some("us-west-2"), ProviderConnectionConfig::default())
                .with_model("anthropic.claude-opus-4-20250514-v1:0");

        assert_eq!(provider.model, "anthropic.claude-opus-4-20250514-v1:0");
    }

    #[test]
    fn test_from_config_with_region_only() {
        let provider = BedrockProvider::from_config(None, Some("eu-west-1"), ProviderConnectionConfig::default());
        assert_eq!(provider.model, DEFAULT_MODEL);
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
        assert_eq!(provider.display_name(), format!("Bedrock ({id})"));
    }

    #[tokio::test]
    async fn separate_inference_profile_arn_is_used_as_request_model_id() {
        let endpoint = FakeBedrockEndpoint::start().await;
        let provider = BedrockProvider::new(ProviderConnectionConfig {
            base_url: Some(endpoint.url.clone()),
            auth_mode: ProviderAuthMode::None,
            request_model: None,
            inference_profile_arn: Some(application_inference_profile_arn().to_string()),
        })
        .await
        .with_model(DEFAULT_MODEL);

        let result = provider.send_converse_stream(&hello_context()).await;
        let request = endpoint.request.await.expect("fake Bedrock endpoint received no request");

        assert!(result.is_err());
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

        assert_eq!(provider.request_model_id(), arn);
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

    struct FakeBedrockEndpoint {
        url: String,
        request: oneshot::Receiver<CapturedRequest>,
    }

    struct CapturedRequest {
        method: Method,
        path: String,
        headers: HeaderMap,
    }

    #[derive(Clone)]
    struct FakeBedrockState {
        request_tx: Arc<Mutex<Option<oneshot::Sender<CapturedRequest>>>>,
        shutdown_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    }

    impl FakeBedrockEndpoint {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind fake Bedrock endpoint");
            let url = format!("http://{}", listener.local_addr().expect("fake Bedrock endpoint address"));
            let (request_tx, request) = oneshot::channel();
            let (shutdown_tx, shutdown) = oneshot::channel();
            let state = FakeBedrockState {
                request_tx: Arc::new(Mutex::new(Some(request_tx))),
                shutdown_tx: Arc::new(Mutex::new(Some(shutdown_tx))),
            };

            let app = Router::new().fallback(any(capture_bedrock_request)).with_state(state);
            tokio::spawn(async move {
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = shutdown.await;
                    })
                    .await
                    .expect("serve fake Bedrock endpoint");
            });

            Self { url, request }
        }
    }

    async fn capture_bedrock_request(
        State(state): State<FakeBedrockState>,
        request: Request<Body>,
    ) -> impl IntoResponse {
        let (parts, _) = request.into_parts();
        if let Some(tx) = state.request_tx.lock().await.take() {
            let _ = tx.send(CapturedRequest {
                method: parts.method,
                path: parts.uri.path().to_string(),
                headers: parts.headers,
            });
        }
        if let Some(tx) = state.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
        (StatusCode::FORBIDDEN, "{}")
    }
}
