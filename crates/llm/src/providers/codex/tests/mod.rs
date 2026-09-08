//! Behavioral tests for the Codex WebSocket transport, run against an
//! in-memory fake Responses server.

mod continuation;
mod errors;
mod reuse;
mod wire;

use super::CodexProvider;
use super::test_server::{self, FakeResponsesWebsocketServer};
use crate::providers::openai_responses::mappers::{ResponsesRequestPolicy, build_wire_request};
use crate::{ChatMessage, Context, LlmResponse, ProviderConnectionConfig, StreamingModelProvider};
use aether_auth::{FakeOAuthCredentialStore, OAuthCredential};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures::StreamExt;
use std::sync::Arc;

const MODEL: &str = "gpt-5.6-luna";

/// The `input` items a request would carry without continuation.
fn full_history(context: &Context) -> Vec<serde_json::Value> {
    let request = build_wire_request(MODEL, context, &ResponsesRequestPolicy::codex()).unwrap();
    request["input"].as_array().cloned().unwrap()
}

async fn collect(provider: &CodexProvider, context: &Context) -> Vec<LlmResponse> {
    provider.stream_response(context).collect::<Vec<_>>().await.into_iter().map(Result::unwrap).collect()
}

fn context(key: &str) -> Context {
    let mut context = Context::new(vec![ChatMessage::user("Hi")], vec![]);
    context.set_session_affinity_key(Some(key.into()));
    context.set_turn_id(Some("turn-1".into()));
    context
}

fn credential(account: &str) -> OAuthCredential {
    let payload = URL_SAFE_NO_PAD
        .encode(serde_json::json!({"https://api.openai.com/auth":{"chatgpt_account_id":account}}).to_string());
    OAuthCredential {
        client_id: "test".into(),
        access_token: format!("e30.{payload}.signature"),
        refresh_token: None,
        expires_at: Some(u64::MAX),
    }
}

fn provider(server: &FakeResponsesWebsocketServer) -> CodexProvider {
    provider_at(&server.base_url)
}

fn provider_at(base_url: &str) -> CodexProvider {
    let store = Arc::new(FakeOAuthCredentialStore::new().with_credential("codex", credential("account-1")));
    CodexProvider::new(store, connection(base_url), MODEL).unwrap()
}

fn connection(base_url: &str) -> ProviderConnectionConfig {
    ProviderConnectionConfig { base_url: Some(base_url.into()), ..Default::default() }
}
