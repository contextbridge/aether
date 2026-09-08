use std::sync::Arc;

use aether_auth::OAuthCredentialStorage;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::redirect::Policy;
use uuid::Uuid;

use super::oauth::CodexTokenManager;
use super::session::{Credentials, InferenceRequest, SessionClient, SessionHandle};
use super::websocket::responses_url;
use crate::provider::{LlmResponseStream, StreamingModelProvider, get_context_window};
use crate::providers::openai_responses::mappers::{ResponsesRequestPolicy, build_typed_request};
use crate::{Context, LlmError, LlmModel, ProviderConnectionConfig, Result};

/// One conversation with at most one active inference request.
///
/// Clones share the same session and overlapping calls are serialized. Consume
/// or drop the active stream before awaiting the next one. Construct a new
/// provider for an independent conversation.
#[derive(Clone)]
pub struct CodexProvider {
    model: String,
    session: Arc<SessionHandle>,
}

impl CodexProvider {
    pub fn new(
        store: Arc<dyn OAuthCredentialStorage>,
        connection: ProviderConnectionConfig,
        model: &str,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .http1_only()
            .redirect(Policy::none())
            .build()
            .map_err(|error| LlmError::HttpClientCreation(error.to_string()))?;
        let endpoint = SessionClient {
            client,
            url: responses_url(&connection.base_url.unwrap_or_else(|| CODEX_API_BASE.to_string()))?,
            model: format!("{}:{model}", super::PROVIDER_ID).parse().ok(),
            token_manager: Arc::new(CodexTokenManager::new(store, super::PROVIDER_ID)),
        };
        Ok(Self { model: model.to_string(), session: Arc::new(SessionHandle::new(endpoint)) })
    }
}

impl StreamingModelProvider for CodexProvider {
    fn model(&self) -> Option<LlmModel> {
        self.session.endpoint().model.clone()
    }

    fn context_window(&self) -> Option<u32> {
        get_context_window(super::PROVIDER_ID, &self.model)
    }

    fn stream_response(&self, context: &Context) -> LlmResponseStream {
        let session = Arc::clone(&self.session);
        let model = self.model.clone();
        let context = context.clone();
        self.session.stream(async move {
            let session_id = session.id(context.session_affinity_key())?;
            let (access_token, account_id) = session.endpoint().token_manager.get_valid_token().await?;
            let policy = ResponsesRequestPolicy::codex();
            let request = build_typed_request(&model, &context, &policy)?;
            Ok(InferenceRequest {
                headers: build_headers(&access_token, &account_id, session_id)?,
                full: request,
                effort: policy.effort(&context),
                identity: Credentials { access_token, account_id },
                turn_id: context.turn_id().map_or_else(new_id, str::to_owned),
            })
        })
    }

    fn display_name(&self) -> String {
        format!("Codex ({})", self.model)
    }
}

const CODEX_API_BASE: &str = "https://chatgpt.com/backend-api/codex";
const CODEX_CLIENT_VERSION: &str = "0.153.4";

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn build_headers(access_token: &str, account_id: &str, session_id: &str) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        (AUTHORIZATION.as_str(), format!("Bearer {access_token}")),
        ("chatgpt-account-id", account_id.into()),
        ("session-id", session_id.into()),
        ("thread-id", session_id.into()),
        ("x-client-request-id", new_id()),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_str(&value)
                .map_err(|_| LlmError::ProviderRequest("Invalid Codex header value".into()))?,
        );
    }
    headers.insert("originator", HeaderValue::from_static("codex_cli_rs"));
    headers.insert("version", HeaderValue::from_static(CODEX_CLIENT_VERSION));
    headers.insert("openai-beta", HeaderValue::from_static("responses_websockets=2026-02-06"));
    Ok(headers)
}
