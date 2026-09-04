use super::oauth::CodexTokenManager;
use crate::provider::{LlmResponseStream, StreamingModelProvider, get_context_window, stream_from};
use crate::providers::openai_responses::mappers::{ResponsesRequestPolicy, build_wire_request};
use crate::providers::openai_responses::transport::{ResponsesConnection, process_connection, send};
use crate::{Context, LlmError, Result};
use aether_auth::OAuthCredentialStorage;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
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

    fn build_wire_request(&self, context: &Context) -> Result<serde_json::Value> {
        build_wire_request(&self.model, context, &ResponsesRequestPolicy::codex())
    }

    async fn build_headers(&self) -> Result<HeaderMap> {
        let (access_token, account_id) = self.token_manager.get_valid_token().await?;

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {access_token}"))
                .map_err(|e| LlmError::ProviderRequest(e.to_string()))?,
        );
        headers.insert(
            "chatgpt-account-id",
            HeaderValue::from_str(&account_id).map_err(|e| LlmError::ProviderRequest(e.to_string()))?,
        );
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));

        Ok(headers)
    }

    /// Send the request and return a stream of SSE lines parsed into typed events.
    ///
    /// Uses manual SSE parsing because the Codex API does not return a
    /// `Content-Type: text/event-stream` header, which `reqwest_eventsource`
    /// (used by `async-openai`'s `create_stream`) requires.
    async fn send_request(&self, request: serde_json::Value, headers: HeaderMap) -> Result<ResponsesConnection> {
        let url = format!("{}/responses", self.base_url);

        debug!("Sending request to Codex API: {url}");
        debug!(
            "Codex request body: {}",
            serde_json::to_string(&request).unwrap_or_else(|_| "<failed to serialize>".to_string())
        );

        match send(&self.client, &url, headers, request).await {
            Ok(connection) => Ok(connection),
            Err(error) => {
                if error.provider().map(|provider| provider.kind) == Some(crate::ProviderErrorKind::Authentication) {
                    self.token_manager.clear_cache().await;
                }
                Err(error)
            }
        }
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
        let context = context.clone();

        stream_from(
            async move {
                let headers = provider.build_headers().await?;
                let request = provider.build_wire_request(&context)?;
                provider.send_request(request, headers).await
            },
            process_connection,
        )
    }

    fn display_name(&self) -> String {
        format!("Codex ({})", self.model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use crate::ToolDefinition;
    use crate::providers::test_capture_server::CaptureServer;
    use aether_auth::{FakeOAuthCredentialStore, OAuthCredential};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use futures::StreamExt;

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
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server).with_model("gpt-5.6-luna");
        let mut context = Context::new(
            vec![ChatMessage::system("You are helpful"), ChatMessage::user("Think harder")],
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
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server);
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

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
