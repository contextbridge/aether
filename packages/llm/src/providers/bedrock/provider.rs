use super::mappers::{map_messages, map_tools};
use super::streaming::process_bedrock_stream;
use crate::provider::{LlmResponseStream, ProviderFactory, StreamingModelProvider, get_context_window};
use crate::{Context, LlmError, Result};
use aws_config::Region;
use aws_sdk_bedrockruntime::config::{BehaviorVersion, Credentials};
use aws_sdk_bedrockruntime::error::SdkError;
use aws_sdk_bedrockruntime::operation::converse_stream::ConverseStreamError;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError;
use aws_sdk_bedrockruntime::types::{ConverseStreamOutput, InferenceConfiguration};
use aws_sdk_bedrockruntime::{Client, Config};
use futures::StreamExt;
use tracing::{error, info};

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
    model: String,
    max_tokens: i32,
    temperature: Option<f32>,
}

impl BedrockProvider {
    /// Create a provider using the default AWS credential chain
    /// (env vars, `~/.aws/credentials`, IAM roles, SSO).
    pub async fn new() -> Self {
        let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
        let client = Client::new(&config);

        Self { client, model: DEFAULT_MODEL.to_string(), max_tokens: DEFAULT_MAX_TOKENS, temperature: None }
    }

    /// Create a provider from explicit configuration without async credential discovery.
    pub fn from_config(credentials: Option<AwsCredentials>, region: Option<&str>) -> Self {
        let client = build_client(credentials, region);

        Self { client, model: DEFAULT_MODEL.to_string(), max_tokens: DEFAULT_MAX_TOKENS, temperature: None }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: i32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_temperature(mut self, temperature: f32) -> Self {
        self.temperature = Some(temperature);
        self
    }

    async fn send_converse_stream(
        &self,
        context: &Context,
    ) -> Result<EventReceiver<ConverseStreamOutput, ConverseStreamOutputError>> {
        let (system_blocks, messages) = map_messages(context.messages())?;
        let mut inference_config = InferenceConfiguration::builder().max_tokens(self.max_tokens);

        if let Some(temp) = self.temperature {
            inference_config = inference_config.temperature(temp);
        }

        let inference_config = inference_config.build();

        let mut request = self
            .client
            .converse_stream()
            .model_id(&self.model)
            .set_messages(Some(messages))
            .inference_config(inference_config);

        if !system_blocks.is_empty() {
            request = request.set_system(Some(system_blocks));
        }

        if !context.tools().is_empty() {
            let tool_config = map_tools(context.tools())?;
            request = request.tool_config(tool_config);
        }

        info!(model = %self.model, "Sending Bedrock converse_stream request");

        let response = request.send().await.map_err(|e| {
            error!(model = %self.model, error = ?e, "Bedrock API error");
            LlmError::from(e)
        })?;

        Ok(response.stream)
    }
}

impl ProviderFactory for BedrockProvider {
    async fn from_env() -> Result<Self> {
        Ok(Self::new().await)
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

        Box::pin(async_stream::stream! {
            match provider.send_converse_stream(&context).await {
                Ok(receiver) => {
                    let mut stream = Box::pin(process_bedrock_stream(receiver));
                    while let Some(result) = stream.next().await {
                        yield result;
                    }
                }
                Err(e) => {
                    yield Err(e);
                }
            }
        })
    }

    fn display_name(&self) -> String {
        format!("Bedrock ({})", self.model)
    }
}

impl From<SdkError<ConverseStreamError>> for LlmError {
    fn from(e: SdkError<ConverseStreamError>) -> Self {
        let message = format!("Bedrock API error: {e}");
        match e {
            SdkError::TimeoutError(_) => LlmError::Timeout(message),
            SdkError::DispatchFailure(_) => LlmError::Network(message),
            SdkError::ResponseError(_) => LlmError::ServerError { status: None, message },
            SdkError::ServiceError(svc) => {
                let inner = svc.err();
                if inner.is_throttling_exception() {
                    LlmError::RateLimited(message)
                } else if inner.is_service_unavailable_exception()
                    || inner.is_internal_server_exception()
                    || inner.is_model_stream_error_exception()
                {
                    LlmError::ServerError { status: None, message }
                } else {
                    LlmError::ApiError(message)
                }
            }
            _ => LlmError::ApiError(message),
        }
    }
}

fn build_client(credentials: Option<AwsCredentials>, region: Option<&str>) -> Client {
    let mut config = Config::builder().behavior_version(BehaviorVersion::latest());

    if let Some(creds) = credentials {
        config = config.credentials_provider(Credentials::new(
            creds.access_key_id,
            creds.secret_access_key,
            creds.session_token,
            None,
            "aether-bedrock-provider",
        ));
    }

    config = config.region(Region::new(region.unwrap_or(DEFAULT_REGION).to_string()));

    Client::from_conf(config.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_provider() -> BedrockProvider {
        BedrockProvider::from_config(None, None)
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
    fn test_with_max_tokens() {
        let provider = test_provider().with_max_tokens(8192);
        assert_eq!(provider.max_tokens, 8192);
    }

    #[test]
    fn test_with_temperature() {
        let provider = test_provider().with_temperature(0.7);
        assert_eq!(provider.temperature, Some(0.7));
    }

    #[test]
    fn test_default_values() {
        let provider = test_provider();
        assert_eq!(provider.model, "anthropic.claude-sonnet-4-5-20250929-v1:0");
        assert_eq!(provider.max_tokens, 16_384);
        assert!(provider.temperature.is_none());
    }

    #[test]
    fn test_from_config_with_credentials() {
        let credentials = AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: None,
        };

        let provider = BedrockProvider::from_config(Some(credentials), None);
        assert_eq!(provider.model, DEFAULT_MODEL);
    }

    #[test]
    fn test_from_config_with_credentials_and_region() {
        let credentials = AwsCredentials {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
            session_token: Some("FwoGZXIvYXdzEBYaD...".to_string()),
        };

        let provider = BedrockProvider::from_config(Some(credentials), Some("us-west-2"))
            .with_model("anthropic.claude-opus-4-20250514-v1:0")
            .with_max_tokens(4096)
            .with_temperature(0.5);

        assert_eq!(provider.model, "anthropic.claude-opus-4-20250514-v1:0");
        assert_eq!(provider.max_tokens, 4096);
        assert_eq!(provider.temperature, Some(0.5));
    }

    #[test]
    fn test_from_config_with_region_only() {
        let provider = BedrockProvider::from_config(None, Some("eu-west-1"));
        assert_eq!(provider.model, DEFAULT_MODEL);
    }
}
