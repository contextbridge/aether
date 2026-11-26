use super::oauth::CodexTokenManager;
use crate::catalog::Provider;
use crate::provider::{LlmResponseStream, StreamingModelProvider, display_name, validate_reasoning};
use crate::provider_connection::DEFAULT_STREAM_IDLE_TIMEOUT;
use crate::providers::http::http_client;
use crate::providers::openai::responses::client::{ResponsesCall, ResponsesClient};
use crate::providers::openai::responses::mappers::{ResponsesRequestPolicy, build_request};
use crate::providers::openai::responses::types::ResponsesRequest;
use crate::{Context, LlmModel, ProviderErrorKind, ResponsesTransport, Result};
use aether_auth::OAuthCredentialStorage;
use futures::TryStreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use std::sync::Arc;

const CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const CODEX_CLIENT_VERSION: &str = "0.167.1";

#[derive(Clone)]
pub struct CodexProvider {
    responses: ResponsesClient,
    model: String,
    request_model: Option<String>,
    token_manager: Arc<CodexTokenManager>,
}

impl CodexProvider {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>) -> Self {
        Self::from_token_manager(CodexTokenManager::new(store, super::PROVIDER_ID))
    }

    pub(crate) fn from_token_manager(token_manager: CodexTokenManager) -> Self {
        Self {
            responses: ResponsesClient::new(
                http_client(),
                CODEX_API_BASE,
                ResponsesTransport::WebSocket,
                DEFAULT_STREAM_IDLE_TIMEOUT,
            ),
            model: "gpt-5.5".to_string(),
            request_model: None,
            token_manager: Arc::new(token_manager),
        }
    }

    pub fn with_connection(mut self, connection: crate::ProviderConnectionConfig) -> Self {
        let transport = connection.responses_transport(ResponsesTransport::WebSocket);
        let base_url = connection.base_url.as_deref().map_or(CODEX_API_BASE, |url| url.trim_end_matches('/'));
        self.responses = ResponsesClient::new(http_client(), base_url, transport, connection.idle_timeout);
        self.request_model = connection.request_model;
        self
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.responses = self.responses.for_another_owner();
        self.model = model.to_string();
        self
    }

    pub async fn forget_cached_token(&self) {
        self.token_manager.clear_cache().await;
    }

    fn build_wire_request(&self, context: &Context) -> Result<ResponsesRequest> {
        let identity = self.model();
        validate_reasoning(context, identity.as_ref())?;
        let wire_model = self.request_model.as_deref().unwrap_or(&self.model);
        build_request(identity.as_ref(), wire_model, context, &ResponsesRequestPolicy::CODEX)
    }

    async fn build_headers(&self, session_affinity_key: Option<&str>) -> Result<HeaderMap> {
        let (access_token, account_id) = self.token_manager.get_valid_token().await?;

        let mut authorization = HeaderValue::from_str(&format!("Bearer {access_token}"))?;
        authorization.set_sensitive(true);

        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert("chatgpt-account-id", HeaderValue::from_str(&account_id)?);
        headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
        headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));
        // ChatGPT's backend derives prompt cache affinity from this header, not from the request body.
        if let Some(key) = session_affinity_key {
            headers.insert("session-id", HeaderValue::from_str(key)?);
        }

        Ok(headers)
    }
}

impl StreamingModelProvider for CodexProvider {
    fn display_name(&self) -> String {
        display_name(Provider::Codex, &self.model)
    }

    fn model(&self) -> Option<LlmModel> {
        LlmModel::parse_for(Provider::Codex, &self.model).ok()
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let provider = self.clone();
        let context = context.clone();
        let token_manager = Arc::clone(&self.token_manager);

        let responses = self.responses.stream(async move {
            let request = provider.build_wire_request(&context)?;
            let headers = provider.build_headers(context.session_affinity_key()).await?;
            Ok(ResponsesCall { request, headers })
        });

        Box::pin(responses.or_else(move |error| {
            let token_manager = Arc::clone(&token_manager);
            async move {
                if error.provider().is_some_and(|provider| provider.kind == ProviderErrorKind::Authentication) {
                    token_manager.clear_cache().await;
                }
                Err(error)
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ChatMessage;
    use crate::LlmError;
    use crate::ToolDefinition;
    use crate::providers::test_capture_server::{CaptureServer, SocketBehavior, hello_context};
    use aether_auth::{MemoryCredentialStore, OAuthCredential};
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use futures::StreamExt;

    #[tokio::test]
    async fn all_codex_models_reject_disabled_before_authentication() {
        for model in
            crate::LlmModel::all().iter().filter(|model| model.provider_enum() == crate::catalog::Provider::Codex)
        {
            assert!(!model.effective_reasoning_levels().contains(&crate::ReasoningEffort::Disabled));
            let provider = CodexProvider::new(Arc::new(MemoryCredentialStore::new())).with_model(&model.model_id());
            let mut context = Context::new(vec![], vec![]);
            context.set_reasoning_effort(crate::ReasoningEffort::Disabled);
            let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
            assert_eq!(responses.len(), 1);
            assert!(matches!(&responses[0], Err(LlmError::ReasoningValidation(_))));
            assert!(!responses[0].as_ref().unwrap_err().is_retryable());
        }
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
        assert_eq!(create_test_provider().with_model("gpt-custom").display_name(), "Codex (gpt-custom)");
    }

    #[tokio::test]
    async fn stream_response_sends_supported_protocol_version_for_gpt_6_luna() {
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server).with_model("gpt-6-luna");
        let mut context = Context::new(
            vec![ChatMessage::system("You are helpful"), ChatMessage::user("Think harder")],
            vec![ToolDefinition::new(
                "bash",
                "Run a command",
                serde_json::from_str(r#"{"type": "object", "properties": {"cmd": {"type": "string"}}}"#).unwrap(),
            )],
        );
        context.set_reasoning_effort(crate::ReasoningEffort::Max);
        context.set_session_affinity_key(Some("session-abc".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert!(captured.body["reasoning"].get("context").is_none());
        assert_eq!(captured.body["model"], "gpt-6-luna");
        assert!(captured.body.get("instructions").is_none());
        assert_eq!(captured.body["tools"].as_array().unwrap().len(), 1);
        assert!(captured.body["tools"][0].get("strict").is_none());
        assert_eq!(captured.body["parallel_tool_calls"], true);
        assert_eq!(captured.body["input"].as_array().unwrap().len(), 2);
        assert_eq!(captured.body["input"][0]["role"], "developer");
        assert_eq!(captured.body["input"][0]["content"], "You are helpful");
        assert_eq!(captured.body["input"][1]["role"], "user");
        assert_eq!(captured.body["prompt_cache_key"], "session-abc");
        assert_eq!(captured.body["store"], false);
        assert_eq!(captured.body["stream"], true);
        assert_eq!(captured.headers["chatgpt-account-id"], "account-1");
        assert_eq!(captured.headers["version"], "0.167.1");
        assert_eq!(captured.headers["accept"], "text/event-stream");
        assert!(captured.headers.get("x-openai-internal-codex-responses-lite").is_none());
        assert!(captured.headers.get("OpenAI-Beta").is_none());
    }

    #[tokio::test]
    async fn session_affinity_routes_the_request_instead_of_the_prefix_key() {
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server);
        let mut context = hello_context();
        context.set_prompt_cache_key(Some("prefix-abc".to_string()));
        context.set_session_affinity_key(Some("session-abc".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.headers["session-id"], "session-abc");
        assert_eq!(captured.body["prompt_cache_key"], "session-abc");
    }

    #[tokio::test]
    async fn requests_without_session_affinity_send_no_routing_identity() {
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server);
        let mut context = hello_context();
        context.set_prompt_cache_key(Some("prefix-abc".to_string()));

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert!(captured.headers.get("session-id").is_none());
        assert!(captured.body.get("prompt_cache_key").is_none());
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
        assert_eq!(captured.body["reasoning"]["summary"], "auto");
    }

    #[tokio::test]
    async fn request_model_routes_the_request_without_changing_catalog_identity() {
        let mut server = CaptureServer::start_responses().await;
        let provider = authenticated_provider().with_connection(crate::ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            request_model: Some("codex-deployment".to_string()),
            ..Default::default()
        });

        let responses = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        let captured = server.captured().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["model"], "codex-deployment");
        assert_eq!(provider.model().unwrap().to_string(), "codex:gpt-5.5");
    }

    #[tokio::test]
    async fn websocket_turns_authenticate_the_handshake_and_continue_the_previous_response() {
        let mut server = CaptureServer::start_responses_with_sockets(SocketBehavior::Remembers).await;
        let store = authenticated_store();
        let provider = CodexProvider::new(store.clone()).with_connection(crate::ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            transport: Some(crate::ResponsesTransport::WebSocket),
            ..Default::default()
        });
        let mut context = hello_context();
        context.set_session_affinity_key(Some("session-abc".to_string()));

        for (account, previous) in [("account-1", None), ("account-1", Some("resp_0_0")), ("account-2", None)] {
            let mut credential = store.load_credential("codex").await.unwrap().unwrap();
            credential.access_token = test_jwt(account);
            store.save_credential("codex", credential).await.unwrap();
            provider.forget_cached_token().await;
            let events = provider.stream_response(&context).collect::<Vec<_>>().await;
            assert!(events.iter().all(Result::is_ok), "{events:?}");
            let [request] = server.drain().try_into().unwrap();
            assert!(request.socket.is_some());
            assert_eq!(request.headers["authorization"], format!("Bearer {}", test_jwt(account)));
            assert_eq!(request.headers["chatgpt-account-id"], account);
            assert_eq!(request.headers["session-id"], "session-abc");
            assert_eq!(request.body["previous_response_id"].as_str(), previous);
            context.add_message(crate::testing::assistant_message().text("hi").build());
            context.add_message(ChatMessage::user("Again?"));
        }
    }

    #[tokio::test]
    async fn a_logout_reaches_a_provider_that_forgets_its_cached_token() {
        let mut server = CaptureServer::start_responses().await;
        let store = authenticated_store();
        let provider = CodexProvider::new(Arc::clone(&store)).with_connection(crate::ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            ..Default::default()
        });

        let before = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;
        server.captured().await;
        store.delete("codex").await.unwrap();
        provider.forget_cached_token().await;
        let after = provider.stream_response(&hello_context()).collect::<Vec<_>>().await;

        assert!(before.iter().all(Result::is_ok), "{before:?}");
        assert!(matches!(after.as_slice(), [Err(LlmError::OAuthError(_))]), "{after:?}");
    }

    fn server_backed_provider(server: &CaptureServer) -> CodexProvider {
        authenticated_provider().with_connection(crate::ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            ..Default::default()
        })
    }

    fn authenticated_provider() -> CodexProvider {
        CodexProvider::new(authenticated_store())
    }

    fn authenticated_store() -> Arc<dyn OAuthCredentialStorage> {
        let credential = OAuthCredential {
            client_id: "test".to_string(),
            access_token: test_jwt("account-1"),
            refresh_token: None,
            expires_at: Some(u64::MAX),
        };
        Arc::new(MemoryCredentialStore::new().with_credential("codex", credential))
    }

    fn test_jwt(account_id: &str) -> String {
        let header = URL_SAFE_NO_PAD.encode(r#"{"alg":"none"}"#);
        let payload = URL_SAFE_NO_PAD
            .encode(serde_json::json!({"https://api.openai.com/auth": {"chatgpt_account_id": account_id}}).to_string());
        format!("{header}.{payload}.signature")
    }

    fn create_test_provider() -> CodexProvider {
        let store: Arc<dyn OAuthCredentialStorage> = Arc::new(MemoryCredentialStore::new());
        CodexProvider::new(store).with_model("gpt-5.5")
    }
}
