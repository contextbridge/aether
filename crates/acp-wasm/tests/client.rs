#![cfg(target_family = "wasm")]

use acp_utils::notifications::RemoteServerInfo;
use acp_wasm::{AetherClient, ClientError};
use agent_client_protocol::schema::v2::{
    ContentBlock, InitializeResponse, NewSessionResponse, PromptResponse, SessionUpdate, StateUpdate, StopReason,
    UpdateSessionNotification,
};
use futures::StreamExt;
use futures::channel::mpsc;
use js_sys::{Array, Function, JSON, Object, Promise, Reflect};
use serde::de::DeserializeOwned;
use serde_json::json;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

const FAKE_AGENT_WS_URL: &str = env!("FAKE_AGENT_WS_URL");

#[wasm_bindgen_test]
async fn initialize_response_advertises_the_remote_server() {
    let client = TestClient::connect("/basic").await;
    let response: InitializeResponse = from_js(client.aether.initialize_response());
    let remote = RemoteServerInfo::from_meta(response.meta.as_ref());
    assert_eq!(remote, Some(RemoteServerInfo { cwd: "/fake/workspace".into(), session_id: None }));
}

#[wasm_bindgen_test]
async fn prompt_resolves_on_acceptance_and_streams_the_turn_as_events() {
    let mut client = TestClient::connect("/basic").await;
    let session: NewSessionResponse =
        resolve(client.aether.new_session(js(&json!({"cwd": "/workspace"})))).await.expect("session is created");
    assert_eq!(session.session_id.0.as_ref(), "fake-session");

    let response = client.prompt().await;
    assert_eq!(response.message_id.0.as_ref(), "user-message");
    assert!(matches!(client.next_update().await, SessionUpdate::StateUpdate(StateUpdate::Running(_))));
    assert_eq!(client.next_message().await, "hello from the fake agent");
    assert!(matches!(
        client.next_update().await,
        SessionUpdate::StateUpdate(StateUpdate::Idle(idle)) if idle.stop_reason == Some(StopReason::EndTurn)
    ));
}

#[wasm_bindgen_test]
async fn elicitation_is_answered_through_the_events_respond_function() {
    let mut client = TestClient::connect("/elicitation").await;
    client.prompt().await;
    assert!(matches!(client.next_update().await, SessionUpdate::StateUpdate(StateUpdate::Running(_))));
    let event = client.next_event().await;
    assert_eq!(event.kind(), "elicitation_request");

    let answer = json!({"action": "accept", "content": {"answer": "yes"}});
    event.respond(&js(&answer)).expect("response is accepted");

    let echo: serde_json::Value = serde_json::from_str(&client.next_message().await).unwrap();
    assert_eq!(echo, answer, "the agent receives exactly what JS answered");
}

#[wasm_bindgen_test]
async fn malformed_elicitation_response_throws_and_answers_invalid_params() {
    let mut client = TestClient::connect("/elicitation").await;
    client.prompt().await;
    client.next_update().await;
    let event = client.next_event().await;

    let error = event.respond(&js(&json!("not a response"))).expect_err("malformed response throws");
    assert_eq!(code(&error), "invalid_argument");

    let echo: serde_json::Value = serde_json::from_str(&client.next_message().await).unwrap();
    assert_eq!(echo["code"], -32602, "the agent is answered with invalid_params");
}

#[wasm_bindgen_test]
async fn a_prompt_reduces_into_conversation_snapshots() {
    let mut client = TestClient::connect("/basic").await;
    resolve::<NewSessionResponse>(client.aether.new_session(js(&json!({"cwd": "/workspace"})))).await.unwrap();
    client.prompt().await;

    let first = client.next_conversation().await;
    let second = client.next_conversation().await;
    assert_eq!(first.json()["turn"], "submitting");
    assert!(Object::is(&first.item(0), &second.item(0)), "an unchanged item keeps its object");

    let settled = client.conversation_where(|conversation| conversation["turn"] == "idle").await;
    let items = settled.json()["items"].clone();
    assert_eq!(items[0]["kind"], "user");
    assert_eq!(items[0]["content"], json!([{"type": "text", "text": "hi"}]));
    assert_eq!(items[1]["kind"], "assistant");
    assert_eq!(items[1]["content"], json!([{"type": "text", "text": "hello from the fake agent"}]));
    assert!(items.as_array().unwrap().iter().all(|item| item["state"] == "sealed"));
    assert_eq!(settled.json()["activity"], json!({"phase": "idle", "thought": ""}));

    let current = client.aether.conversation("fake-session".into()).unwrap();
    assert_eq!(from_js::<serde_json::Value>(current), settled.json());
}

#[wasm_bindgen_test]
async fn a_prompt_during_a_turn_throws_turn_in_progress() {
    let client = TestClient::connect("/elicitation").await;
    client.prompt().await;
    let error = client
        .aether
        .prompt(js(&json!({"sessionId": "fake-session", "prompt": [{"type": "text", "text": "again"}]})))
        .expect_err("the elicitation holds the turn open");
    assert_eq!(error.code(), "turn_in_progress");
}

#[wasm_bindgen_test]
async fn resuming_with_replay_rebuilds_the_conversation() {
    let mut client = TestClient::connect("/basic").await;
    client.prompt().await;
    client.conversation_where(|conversation| conversation["turn"] == "idle").await;

    let request = js(&json!({"sessionId": "fake-session", "cwd": "/workspace"}));
    JsFuture::from(client.aether.resume_session(request, true).unwrap()).await.expect("session resumes");

    let restarted = client.next_conversation().await;
    assert_eq!(restarted.json()["items"], json!([]), "resuming starts the conversation over");
    let current = from_js::<serde_json::Value>(client.aether.conversation("fake-session".into()).unwrap());
    assert_eq!(current["turn"], "idle");
    assert_eq!(current["items"].as_array().unwrap().len(), 1);
    assert_eq!(current["items"][0]["content"], json!([{"type": "text", "text": "saved answer"}]));
}

#[wasm_bindgen_test]
async fn an_unseen_session_has_no_conversation() {
    let client = TestClient::connect("/basic").await;
    assert!(client.aether.conversation("unknown".into()).unwrap().is_undefined());
}

#[wasm_bindgen_test]
async fn malformed_request_throws_invalid_argument() {
    let client = TestClient::connect("/basic").await;
    let error = client.aether.prompt(js(&json!({"prompt": "not a prompt"}))).expect_err("malformed request throws");
    assert_eq!(error.code(), "invalid_argument");
}

#[wasm_bindgen_test]
async fn disconnect_emits_connection_closed_and_later_requests_reject() {
    let mut client = TestClient::connect("/basic").await;
    JsFuture::from(client.aether.disconnect()).await.expect("disconnect resolves");
    let event = client.next_event().await;
    assert_eq!(event.kind(), "connection_closed");
    assert!(event.get("close").is_null(), "this client closed the socket");

    let error = resolve::<NewSessionResponse>(client.aether.new_session(js(&json!({"cwd": "/workspace"}))))
        .await
        .expect_err("requests after close reject");
    assert_eq!(code(&error), "protocol");
    let error = client.aether.cancel("fake-session".into()).expect_err("notifications after close throw");
    assert_eq!(error.code(), "protocol");
}

#[wasm_bindgen_test]
async fn server_close_emits_connection_closed_with_its_code_and_reason() {
    let mut client = TestClient::connect("/close").await;
    resolve::<NewSessionResponse>(client.aether.new_session(js(&json!({"cwd": "/workspace"}))))
        .await
        .expect_err("the server closes instead of answering");
    let event = client.next_event().await;
    assert_eq!(event.kind(), "connection_closed");
    assert_eq!(
        from_js::<serde_json::Value>(event.get("close")),
        json!({"code": 4000, "reason": "closed by the fake agent"})
    );
}

#[wasm_bindgen_test]
async fn connect_failure_rejects_with_connect_failed_and_the_abnormal_close() {
    let error = connect_error("ws://127.0.0.1:1".into(), JsValue::UNDEFINED).await;
    assert_eq!(code(&error), "connect_failed");
    assert_eq!(close(&error)["code"], 1006);
}

#[wasm_bindgen_test]
async fn close_before_initialize_rejects_with_the_servers_close() {
    let error = connect_error(url("/attached"), JsValue::UNDEFINED).await;
    assert_eq!(code(&error), "connect_failed");
    assert_eq!(close(&error), json!({"code": 4409, "reason": "another client is attached"}));
}

#[wasm_bindgen_test]
async fn protocols_option_offers_websocket_subprotocols() {
    let options = js(&json!({"protocols": ["fake-agent.v1"]}));
    let client = TestClient::connect_with("/protocol", options).await;
    let response: InitializeResponse = from_js(client.aether.initialize_response());
    assert!(response.meta.is_some(), "the gateway accepted the subprotocol and the agent initialized");
}

#[wasm_bindgen_test]
async fn malformed_options_reject_with_invalid_argument() {
    let error = connect_error(url("/basic"), js(&json!({"protocols": "fake-agent.v1"}))).await;
    assert_eq!(code(&error), "invalid_argument");
}

struct TestClient {
    aether: AetherClient,
    events: mpsc::UnboundedReceiver<JsValue>,
    conversations: mpsc::UnboundedReceiver<JsValue>,
    _on_event: Closure<dyn FnMut(JsValue)>,
}

impl TestClient {
    async fn connect(scenario: &str) -> Self {
        Self::connect_with(scenario, JsValue::UNDEFINED).await
    }

    async fn connect_with(scenario: &str, options: JsValue) -> Self {
        let (events_tx, events) = mpsc::unbounded();
        let (conversations_tx, conversations) = mpsc::unbounded();
        let on_event = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let channel =
                if Event(event.clone()).kind() == "conversation_changed" { &conversations_tx } else { &events_tx };
            let _ = channel.unbounded_send(event);
        });
        let aether =
            AetherClient::connect(url(scenario), on_event.as_ref().unchecked_ref::<Function>().clone(), options)
                .await
                .expect("connects to the fake agent");
        Self { aether, events, conversations, _on_event: on_event }
    }

    async fn prompt(&self) -> PromptResponse {
        let request = js(&json!({"sessionId": "fake-session", "prompt": [{"type": "text", "text": "hi"}]}));
        resolve(self.aether.prompt(request)).await.expect("prompt is accepted")
    }

    async fn next_event(&mut self) -> Event {
        Event(self.events.next().await.expect("event stream ended"))
    }

    /// The next `conversation_changed` snapshot.
    async fn next_conversation(&mut self) -> Snapshot {
        let event = Event(self.conversations.next().await.expect("event stream ended"));
        assert_eq!(event.get("sessionId").as_string().as_deref(), Some("fake-session"));
        Snapshot(event.get("conversation"))
    }

    async fn conversation_where(&mut self, predicate: impl Fn(&serde_json::Value) -> bool) -> Snapshot {
        loop {
            let snapshot = self.next_conversation().await;
            if predicate(&snapshot.json()) {
                return snapshot;
            }
        }
    }

    async fn next_update(&mut self) -> SessionUpdate {
        let event = self.next_event().await;
        assert_eq!(event.kind(), "session_update");
        from_js::<UpdateSessionNotification>(event.get("notification")).update
    }

    async fn next_message(&mut self) -> String {
        let SessionUpdate::AgentMessageChunk(chunk) = self.next_update().await else {
            panic!("expected an agent message chunk");
        };
        let ContentBlock::Text(text) = chunk.content else {
            panic!("expected text content");
        };
        text.text
    }
}

struct Event(JsValue);

struct Snapshot(JsValue);

impl Snapshot {
    fn json(&self) -> serde_json::Value {
        from_js(self.0.clone())
    }

    fn item(&self, index: u32) -> JsValue {
        Reflect::get(&self.0, &"items".into()).unwrap().unchecked_into::<Array>().get(index)
    }
}

impl Event {
    fn get(&self, field: &str) -> JsValue {
        Reflect::get(&self.0, &field.into()).unwrap()
    }

    fn kind(&self) -> String {
        self.get("type").as_string().unwrap()
    }

    fn respond(&self, response: &JsValue) -> Result<JsValue, JsValue> {
        self.get("respond").unchecked_into::<Function>().call1(&JsValue::NULL, response)
    }
}

async fn resolve<T: DeserializeOwned>(promise: Result<Promise, ClientError>) -> Result<T, JsValue> {
    JsFuture::from(promise.expect("request is well-formed")).await.map(from_js)
}

fn js(value: &serde_json::Value) -> JsValue {
    JSON::parse(&value.to_string()).unwrap()
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> T {
    serde_wasm_bindgen::from_value(value).unwrap()
}

async fn connect_error(url: String, options: JsValue) -> JsValue {
    AetherClient::connect(url, Function::new_no_args(""), options).await.err().expect("connect fails").into()
}

fn url(scenario: &str) -> String {
    format!("{FAKE_AGENT_WS_URL}{scenario}")
}

fn code(error: &JsValue) -> String {
    Reflect::get(error, &"code".into()).unwrap().as_string().unwrap()
}

fn close(error: &JsValue) -> serde_json::Value {
    from_js(Reflect::get(error, &"close".into()).unwrap())
}
