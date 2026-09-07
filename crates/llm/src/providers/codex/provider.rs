use super::oauth::CodexTokenManager;
use crate::provider::{LlmResponseStream, StreamingModelProvider, get_context_window};
use crate::providers::openai_responses::mappers::ResponsesRequestPolicy;
use crate::providers::openai_responses::websocket::{
    AuthenticationFailureHook, WsRequestParams, derive_ws_url, stream_via_websocket,
};
use crate::{Context, LlmError, Result};
use aether_auth::OAuthCredentialStorage;
use futures::StreamExt;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use std::sync::Arc;

const CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const CODEX_CLIENT_VERSION: &str = "0.153.4";

#[derive(Clone)]
pub struct CodexProvider {
    base_url: String,
    model: String,
    token_manager: Arc<CodexTokenManager>,
}

impl CodexProvider {
    pub fn new(store: Arc<dyn OAuthCredentialStorage>) -> Self {
        let token_manager = CodexTokenManager::new(store, super::PROVIDER_ID);
        Self {
            base_url: CODEX_API_BASE.to_string(),
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

    /// Route a turn over the persistent WebSocket: same credentials and
    /// headers the HTTP path used, carried on the `wss://` handshake. An
    /// authentication failure at handshake drops the cached token so the
    /// next turn re-authenticates from storage.
    async fn websocket_params(&self) -> Result<WsRequestParams> {
        let handshake_headers = self.build_headers().await?;
        let ws_url = derive_ws_url(&format!("{}/responses", self.base_url))?;
        let token_manager = Arc::clone(&self.token_manager);
        let on_authentication_failure: AuthenticationFailureHook = Arc::new(move || {
            let token_manager = Arc::clone(&token_manager);
            Box::pin(async move { token_manager.clear_cache().await })
        });
        Ok(WsRequestParams {
            ws_url,
            handshake_headers,
            policy: ResponsesRequestPolicy::codex(),
            on_authentication_failure: Some(on_authentication_failure),
        })
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

        Box::pin(async_stream::stream! {
            let params = match provider.websocket_params().await {
                Ok(params) => params,
                Err(error) => {
                    yield Err(error);
                    return;
                }
            };
            let mut turn = stream_via_websocket(params, provider.model.clone(), context);
            while let Some(item) = turn.next().await {
                yield item;
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
        let captured = server.captured_ws().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["type"], "response.create");
        assert_eq!(captured.body["reasoning"]["effort"], "max");
        assert!(captured.body["reasoning"].get("context").is_none());
        assert_eq!(captured.body["model"], "gpt-5.6-luna");
        assert_eq!(captured.body["instructions"], "You are helpful");
        assert_eq!(captured.body["tools"].as_array().unwrap().len(), 1);
        assert!(captured.body.get("parallel_tool_calls").is_none());
        assert_eq!(captured.body["input"][0]["role"], "user");
        assert_eq!(captured.body["prompt_cache_key"], "session-abc");
        assert_eq!(captured.body["store"], false);
        assert!(captured.body.get("stream").is_none(), "WebSocket envelopes must not carry `stream`");
        let authorization = captured.headers["authorization"].to_str().unwrap();
        assert!(authorization.starts_with("Bearer "), "{authorization}");
        assert_eq!(captured.headers["chatgpt-account-id"], "account-1");
        assert_eq!(captured.headers["originator"], "codex_cli_rs");
        assert_eq!(captured.headers["version"], "0.153.4");
    }

    #[tokio::test]
    async fn stream_response_defaults_to_medium_effort_on_the_wire() {
        let mut server = CaptureServer::start_responses().await;
        let provider = server_backed_provider(&server);
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let captured = server.captured_ws().await;

        assert!(responses.iter().all(Result::is_ok), "{responses:?}");
        assert_eq!(captured.body["reasoning"]["effort"], "medium");
    }

    #[tokio::test]
    async fn unauthorized_handshakes_clear_the_cached_token() {
        let mut server = CaptureServer::start_responses().await;
        server.reject_ws_handshake();
        let credential = OAuthCredential {
            client_id: "test".to_string(),
            access_token: test_jwt("account-1"),
            refresh_token: None,
            expires_at: Some(u64::MAX),
        };
        let store: Arc<dyn OAuthCredentialStorage> =
            Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", credential));
        let provider = CodexProvider::new(store.clone()).with_connection(crate::ProviderConnectionConfig {
            base_url: Some(server.base_url.clone()),
            ..Default::default()
        });
        let context = Context::new(vec![ChatMessage::user("Hello")], vec![]);

        let responses = provider.stream_response(&context).collect::<Vec<_>>().await;
        let error = responses.iter().find_map(|r| r.as_ref().err()).expect("expected a failure");
        assert_eq!(error.provider().expect("expected provider error").kind, crate::ProviderErrorKind::Authentication);

        // Rotate the stored credential: the next handshake must present the
        // new token, proving the stale cached token was dropped.
        store
            .save_credential(
                "codex",
                OAuthCredential {
                    client_id: "test".to_string(),
                    access_token: test_jwt("account-2"),
                    refresh_token: None,
                    expires_at: Some(u64::MAX),
                },
            )
            .await
            .unwrap();
        server.allow_ws_handshake();

        let retried = provider.stream_response(&context).collect::<Vec<_>>().await;
        assert!(retried.iter().all(Result::is_ok), "{retried:?}");
        let captured = server.captured_ws().await;
        let authorization = captured.headers["authorization"].to_str().unwrap();
        assert_eq!(captured.headers["chatgpt-account-id"], "account-2");
        assert!(!authorization.contains(&test_jwt("account-1")));
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
