use super::mappers::{map_messages, map_tools};
use super::oauth::CodexTokenManager;
use super::streaming::process_response_stream;
use crate::provider::{LlmResponseStream, StreamingModelProvider, get_context_window};
use crate::{Context, LlmError, Result};
use aether_auth::OAuthCredentialStorage;
use async_openai::types::responses::{
    CreateResponse, IncludeEnum, InputParam, Reasoning, ReasoningEffort, ReasoningSummary, ResponseStreamEvent,
    ResponseTextParam, TextResponseFormatConfiguration, Verbosity,
};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use std::sync::Arc;
use tracing::debug;

const CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";

#[derive(Clone)]
pub struct CodexProvider {
    client: reqwest::Client,
    model: String,
    token_manager: Arc<CodexTokenManager>,
}

impl CodexProvider {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>) -> Self {
        let token_manager = CodexTokenManager::new(store, super::PROVIDER_ID);
        Self { client: reqwest::Client::new(), model: "gpt-5.5".to_string(), token_manager: Arc::new(token_manager) }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = model.to_string();
        self
    }

    fn build_request(&self, context: &Context) -> Result<CreateResponse> {
        let (system_prompt, input) = map_messages(context.messages())?;
        let tools = if context.tools().is_empty() { None } else { Some(map_tools(context.tools())?) };

        let codex_effort = context.reasoning_effort().map_or(ReasoningEffort::Medium, to_codex_effort);

        Ok(CreateResponse {
            model: Some(self.model.clone()),
            input: InputParam::Items(input),
            instructions: system_prompt,
            tools,
            store: Some(false),
            stream: Some(true),
            reasoning: Some(Reasoning { effort: Some(codex_effort), summary: Some(ReasoningSummary::Auto) }),
            include: Some(vec![IncludeEnum::ReasoningEncryptedContent]),
            text: Some(ResponseTextParam {
                format: TextResponseFormatConfiguration::Text,
                verbosity: Some(Verbosity::Medium),
            }),
            prompt_cache_key: context.prompt_cache_key().map(String::from),
            ..Default::default()
        })
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
        headers.insert("OpenAI-Beta", HeaderValue::from_static("responses=experimental"));
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));

        Ok(headers)
    }

    /// Send the request and return a stream of SSE lines parsed into typed events.
    ///
    /// Uses manual SSE parsing because the Codex API does not return a
    /// `Content-Type: text/event-stream` header, which `reqwest_eventsource`
    /// (used by `async-openai`'s `create_stream`) requires.
    async fn send_request(
        &self,
        request: CreateResponse,
        headers: HeaderMap,
    ) -> Result<impl futures::Stream<Item = Result<ResponseStreamEvent>>> {
        let url = format!("{CODEX_API_BASE}/responses");

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
                Ok(event) => match serde_json::from_str::<ResponseStreamEvent>(&event.data) {
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

            let request = match provider.build_request(&context) {
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

fn to_codex_effort(effort: crate::ReasoningEffort) -> ReasoningEffort {
    match effort {
        crate::ReasoningEffort::Low => ReasoningEffort::Low,
        crate::ReasoningEffort::Medium => ReasoningEffort::Medium,
        crate::ReasoningEffort::High => ReasoningEffort::High,
        crate::ReasoningEffort::Xhigh => ReasoningEffort::Xhigh,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use crate::ContentBlock;
    use crate::ToolDefinition;
    use crate::types::IsoString;
    use aether_auth::FakeOAuthCredentialStore;

    #[test]
    fn build_request_simple() {
        let provider = create_test_provider();
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() }],
            vec![],
        );

        let request = provider.build_request(&context).unwrap();
        assert_eq!(request.model.as_deref(), Some("gpt-5.5"));
        assert_eq!(request.store, Some(false));
        assert_eq!(request.stream, Some(true));
        assert!(request.tools.is_none());
        assert!(request.instructions.is_none());
        if let InputParam::Items(items) = &request.input {
            assert_eq!(items.len(), 1);
        } else {
            panic!("Expected InputParam::Items");
        }
    }

    #[test]
    fn build_request_with_system_and_tools() {
        let provider = create_test_provider();
        let context = Context::new(
            vec![
                ChatMessage::System { content: "You are helpful".to_string(), timestamp: IsoString::now() },
                ChatMessage::User { content: vec![ContentBlock::text("Hello")], timestamp: IsoString::now() },
            ],
            vec![ToolDefinition {
                name: "bash".to_string(),
                description: "Run a command".to_string(),
                parameters: r#"{"type": "object", "properties": {"cmd": {"type": "string"}}}"#.to_string(),
                server: None,
            }],
        );

        let request = provider.build_request(&context).unwrap();
        assert!(request.instructions.is_some());
        if let InputParam::Items(items) = &request.input {
            assert_eq!(items.len(), 1);
        } else {
            panic!("Expected InputParam::Items");
        }
        assert!(request.tools.is_some());
        assert_eq!(request.tools.as_ref().unwrap().len(), 1);
    }

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

    #[test]
    fn build_request_defaults_to_medium_effort() {
        let provider = create_test_provider();
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hi")], timestamp: IsoString::now() }],
            vec![],
        );

        let request = provider.build_request(&context).unwrap();
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["reasoning"]["effort"], "medium");
    }

    #[test]
    fn build_request_uses_context_reasoning_effort() {
        let provider = create_test_provider();
        let mut context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Think hard")], timestamp: IsoString::now() }],
            vec![],
        );
        context.set_reasoning_effort(Some(crate::ReasoningEffort::High));

        let request = provider.build_request(&context).unwrap();
        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["reasoning"]["effort"], "high");
    }

    #[test]
    fn build_request_serializes_correctly() {
        let provider = create_test_provider();
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hi")], timestamp: IsoString::now() }],
            vec![],
        );

        let request = provider.build_request(&context).unwrap();
        let json = serde_json::to_value(&request).unwrap();

        assert_eq!(json["model"], "gpt-5.5");
        assert_eq!(json["store"], false);
        assert_eq!(json["stream"], true);
        assert_eq!(json["reasoning"]["effort"], "medium");
        assert_eq!(json["text"]["verbosity"], "medium");
        assert_eq!(json["include"][0], "reasoning.encrypted_content");
    }

    #[test]
    fn build_request_includes_prompt_cache_key_when_set() {
        let provider = create_test_provider();
        let mut context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hi")], timestamp: IsoString::now() }],
            vec![],
        );
        context.set_prompt_cache_key(Some("session-abc".to_string()));

        let request = provider.build_request(&context).unwrap();
        assert_eq!(request.prompt_cache_key.as_deref(), Some("session-abc"));
    }

    #[test]
    fn build_request_omits_prompt_cache_key_when_unset() {
        let provider = create_test_provider();
        let context = Context::new(
            vec![ChatMessage::User { content: vec![ContentBlock::text("Hi")], timestamp: IsoString::now() }],
            vec![],
        );

        let request = provider.build_request(&context).unwrap();
        assert!(request.prompt_cache_key.is_none());
    }

    fn create_test_provider() -> CodexProvider {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(FakeOAuthCredentialStore::new());
        CodexProvider::new(store).with_model("gpt-5.5")
    }
}
