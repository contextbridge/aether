use super::mappers::{map_messages, map_tools};
use super::oauth::CodexTokenManager;
use super::streaming::{CodexStreamEvent, process_response_stream};
use crate::provider::{LlmResponseStream, StreamingModelProvider, get_context_window};
use crate::{Context, LlmError, Result};
use aether_auth::OAuthCredentialStorage;
use async_openai::types::responses::{
    CreateResponse, IncludeEnum, InputParam, Reasoning, ReasoningSummary, ResponseTextParam,
    TextResponseFormatConfiguration, Verbosity,
};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderValue};
use serde_json::Value;
use std::sync::Arc;
use tracing::debug;

const CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const CODEX_CLIENT_VERSION: &str = "0.144.0";

#[derive(Clone)]
pub struct CodexProvider {
    base_url: String,
    client: reqwest::Client,
    model: String,
    token_manager: Arc<CodexTokenManager>,
}

impl CodexProvider {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>) -> Self {
        let token_manager = CodexTokenManager::new(store, super::PROVIDER_ID);
        Self {
            base_url: CODEX_API_BASE.to_string(),
            client: reqwest::Client::new(),
            model: "gpt-5.5".to_string(),
            token_manager: Arc::new(token_manager),
        }
    }

    pub fn with_connection(mut self, connection: crate::ProviderConnectionConfig) -> Self {
        if let Some(base_url) = connection.base_url {
            self.base_url = base_url.trim_end_matches('/').to_string();
        }
        self
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    fn build_typed_request(&self, context: &Context) -> Result<CreateResponse> {
        let (system_prompt, input) = map_messages(context.messages())?;
        let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools())?) };

        Ok(CreateResponse {
            model: Some(self.model.clone()),
            input: InputParam::Items(input),
            instructions: system_prompt,
            tools,
            store: Some(false),
            stream: Some(true),
            reasoning: Some(Reasoning { effort: None, summary: Some(ReasoningSummary::Auto) }),
            include: Some(vec![IncludeEnum::ReasoningEncryptedContent]),
            text: Some(ResponseTextParam {
                format: TextResponseFormatConfiguration::Text,
                verbosity: Some(Verbosity::Medium),
            }),
            prompt_cache_key: context.prompt_cache_key().map(String::from),
            ..Default::default()
        })
    }

    /// The typed `async-openai` request cannot encode every effort the Codex
    /// API accepts (e.g. `max`), so the effort is written onto the serialized
    /// wire body instead.
    fn build_wire_request(&self, context: &Context) -> Result<Value> {
        let mut body = serde_json::to_value(self.build_typed_request(context)?)?;
        let effort = context.reasoning_effort().map_or("medium", crate::ReasoningEffort::as_str);
        body["reasoning"]["effort"] = effort.into();
        Ok(body)
    }

    async fn build_headers(&self) -> Result<HeaderMap> {
        let (access_token, account_id) = self.token_manager.get_valid_token().await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access_token}"))
                .map_err(|e| LlmError::InvalidApiKey(e.to_string()))?,
        );
        headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_str(&account_id).map_err(|e| LlmError::InvalidApiKey(e.to_string()))?,
        );
        headers.insert(ACCEPT, HeaderValue::from_static("text/event-stream"));
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));

        Ok(headers)
    }

    /// Send the request and return a stream of SSE lines parsed into typed events.
    ///
    /// Uses manual SSE parsing because the Codex API does not return a
    /// `Content-Type: text/event-stream` header, which `reqwest_eventsource`
    /// (used by `async-openai`'s `create_stream`) requires.
    async fn send_request(
        &self,
        request: Value,
        headers: HeaderMap,
    ) -> Result<impl futures::Stream<Item = Result<CodexStreamEvent>>> {
        let url = format!("{}/responses", self.base_url);

        debug!("Sending request to Codex API: {url}");
        debug!(
            "Codex request body: {}",
            serde_json::to_string(&request).unwrap_or_else(|_| "<failed to serialize>".to_string())
        );

        let response = self.client.post(&url).headers(headers).json(&request).send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());

            if matches!(status.as_u16(), 401 | 403) {
                self.token_manager.clear_cache().await;
            }

            let message = format!("Codex API request failed with status {status}: {error_text}");
            return Err(match status.as_u16() {
                429 => LlmError::RateLimited(message),
                s if (500..600).contains(&s) => LlmError::ServerError { status: Some(s), message },
                _ => LlmError::ApiError(message),
            });
        }

        let event_stream = response.bytes_stream().eventsource().filter_map(|result| {
            std::future::ready(match result {
                Ok(event) if event.data == "[DONE]" => None,
                Ok(event) => match serde_json::from_str::<CodexStreamEvent>(&event.data) {
                    Ok(parsed) => Some(Ok(parsed)),
                    Err(e) => {
                        debug!("Failed to parse Codex SSE line: {} - Error: {e}", event.data);
                        None
                    }
                },
                Err(e) => Some(Err(LlmError::StreamInterrupted(e.to_string()))),
            })
        });

        Ok(event_stream)
    }
}

impl StreamingModelProvider for CodexProvider {
    fn model(&self) -> Option<crate::LlmModel> {
        format!("{}:{}", super::PROVIDER_ID, self.model).parse().ok()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window(super::PROVIDER_ID, &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let provider = self.clone();
        let context = match self.model() {
            Some(model) => context.filter_encrypted_reasoning(&model),
            None => context.clone(),
        };

        Box::pin(async_stream::stream! {
            let headers = match provider.build_headers().await {
                Ok(h) => h,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let request = match provider.build_wire_request(&context) {
                Ok(r) => r,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let event_stream = match provider.send_request(request, headers).await {
                Ok(s) => s,
                Err(e) => {
                    yield Err(e);
                    return;
                }
            };

            let mut response_stream = Box::pin(process_response_stream(event_stream));
            while let Some(result) = response_stream.next().await {
                yield result;
            }
        })
    }

    fn display_name(&self) -> String {
        format!("Codex ({})", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use crate::ContentBlock;
    use crate::ToolDefinition;
    use crate::providers::test_capture_server::CaptureServer;
    use crate::types::IsoString;
    use aether_auth::{FakeOAuthCredentialStore, OAuthCredential};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;

    #[test]
    fn context_window_uses_codex_subscription_limit() {
        let provider = create_test_provider();
        assert_eq!(provider.context_window(), Some(272_000));
    }

    #[test]
    fn display_name_includes_model() {
        let provider = create_test_provider();
        assert_eq!(provider.display_name(), "Codex (gpt-5.5)");
    }

    #[tokio::test]
    async fn stream_response_sends_supported_protocol_version_for_gpt_5_6_luna() {
        let mut server = CaptureServer::start().await;
        let provider = server_backed_provider(&server).with_model("gpt-5.6-luna");
        let mut context = Context::new(
            vec![
                ChatMessage::System { content: "You are helpful".to_string(), timestamp: IsoString::now() },
                ChatMessage::User { content: vec![ContentBlock::text("Think harder")], timestamp: IsoString::now() },
            ],
            vec![ToolDefinition::new(
                "bash",
                "Run a command",
                serde_json::from_str(r#"{"type": "object", "properties": {"cmd": {"type": "string"}}}"#).unwrap(),
            )],
        );
        context.set_reasoning_effort(Some(crate::ReasoningEffort::Max));
        context.set_prompt_cache_key(Some("session-abc".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert!(captured.body["reasoning"].get("context").is_none());
        assert_eq!(captured.body["model"], "gpt-5.6-luna");
        assert_eq!(captured.body["instructions"], "You are helpful");
        assert_eq!(captured.body["tools"].as_array().unwrap().len(), 1);
        assert!(captured.body.get("parallel_tool_calls").is_none());
        assert_eq!(captured.body["input"][0]["role"], "user");
        assert_eq!(captured.body["prompt_cache_key"], "session-abc");
        assert_eq!(captured.body["store"], false);
        assert_eq!(captured.body["stream"], true);
        assert_eq!(captured.headers["chatgpt-account-id"], "account-1");
        assert_eq!(captured.headers["version"], "0.144.0");
        assert_eq!(captured.headers["accept"], "text/event-stream");
        assert!(captured.headers.get("x-openai-internal-codex-responses-lite").is_none());
        assert!(captured.headers.get("OpenAI-Beta").is_none());
    }

    #[tokio::test]
    async fn stream_response_defaults_to_medium_effort_on_the_wire() {
        let mut server = CaptureServer::start().await;
        let provider = server_backed_provider(&server);
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() }],
            vec![],
        );

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "medium");
    }

    fn server_backed_provider(server: &CaptureServer) -> CodexProvider {
        let credential = OAuthCredential {
            client_id: "test".to_string(),
            access_token: test_jwt("account-1"),
            refresh_token: None,
            expires_at: Some(u64::MAX),
            granted_scopes: Vec::new(),
        };
        let store: Arc<dyn OAuthCredentialStorage> =
            Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", credential));
        CodexProvider::new(store).with_connection(crate::ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            ..Default::default()
        })
    }

    fn test_jwt(account_id: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::json!({"https://api.openai.com/auth": {"chatgpt_account_id": account_id}}).to_string());
        format!("{header}.{payload}.signature")
    }

    fn create_test_provider() -> CodexProvider {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        CodexProvider::new(store).with_model("gpt-5.5")
    }
}
