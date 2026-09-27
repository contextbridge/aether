#![cfg(target_family = "wasm")]

mod common;

use acp_utils::notifications::RemoteServerInfo;
use aether_browser::types::AetherClientErrorCode;
use aether_browser::{AetherClient, Elicitation};
use agent_client_protocol::schema::v2::{
    ContentBlock, CreateElicitationRequest, InitializeResponse, NewSessionResponse, PromptResponse, SessionUpdate,
    StateUpdate, StopReason, UpdateSessionNotification,
};
use futures::StreamExt;
use futures::channel::mpsc;
use js_sys::{Array, Function, JSON, Object, Promise, Reflect};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::iter;
use wasm_bindgen::JsCast;
use wasm_bindgen::convert::TryFromJsValue;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::wasm_bindgen_test;

#[wasm_bindgen_test]
async fn remote_is_the_server_the_initialize_response_advertises() {
    let client = test_client().connect().await;
    let remote: Option<RemoteServerInfo> = from_js(client.aether.remote());
    assert_eq!(remote, Some(RemoteServerInfo { cwd: "/fake/workspace".into(), session_id: None }));
}

#[wasm_bindgen_test]
async fn new_session_emits_its_empty_conversation_before_resolving() {
    let mut client = test_client().connect().await;

    let response = client.new_session().await.expect("session is created");
    assert_eq!(response.session_id.0.as_ref(), "fake-session");
    let conversation = client.latest_conversation().json();
    assert_eq!(conversation["sessionId"], "fake-session");
    assert_eq!(conversation["items"], json!([]));
}

#[wasm_bindgen_test]
async fn without_a_session_prompt_cancel_and_close_reject_with_no_session() {
    let client = test_client().connect().await;
    let error = client.try_prompt().await.expect_err("prompt needs a session");
    assert_eq!(code(&error), "no_session");
    let error = client.cancel().await.expect_err("cancel needs a session");
    assert_eq!(code(&error), "no_session");
    let error = client.close_session().await.expect_err("close needs a session");
    assert_eq!(code(&error), "no_session");
}

#[wasm_bindgen_test]
async fn prompt_resolves_on_acceptance_and_streams_the_turn_as_events() {
    let mut client = test_client().with_session().connect().await;

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
async fn elicitation_is_answered_through_its_respond_method() {
    let mut client = test_client().eliciting().with_session().connect().await;
    let mut elicitation = client.elicit().await;
    let request: CreateElicitationRequest = from_js(elicitation.request());
    assert_eq!(request.message, "Continue?");

    let answer = json!({"action": "accept", "content": {"answer": "yes"}});
    elicitation.respond(js(&answer)).expect("response is accepted");

    let echo: serde_json::Value = serde_json::from_str(&client.next_message().await).unwrap();
    assert_eq!(echo, answer, "the agent receives exactly what JS answered");
}

#[wasm_bindgen_test]
async fn a_second_answer_throws_already_answered() {
    let mut client = test_client().eliciting().with_session().connect().await;
    let mut elicitation = client.elicit().await;
    elicitation.respond(js(&json!({"action": "decline"}))).expect("the first answer is accepted");

    let error = elicitation.respond(js(&json!({"action": "accept"}))).expect_err("the second answer throws");
    assert_eq!(error.details().code, AetherClientErrorCode::AlreadyAnswered);
}

#[wasm_bindgen_test]
async fn malformed_elicitation_response_throws_and_leaves_it_unanswered() {
    let mut client = test_client().eliciting().with_session().connect().await;
    let mut elicitation = client.elicit().await;

    let error = elicitation.respond(js(&json!("not a response"))).expect_err("malformed response throws");
    assert_eq!(error.details().code, AetherClientErrorCode::InvalidArgument);

    let answer = json!({"action": "decline"});
    elicitation.respond(js(&answer)).expect("a well-formed answer is accepted");
    let echo: serde_json::Value = serde_json::from_str(&client.next_message().await).unwrap();
    assert_eq!(echo, answer);
}

#[wasm_bindgen_test]
async fn releasing_an_unanswered_elicitation_answers_cancel() {
    let mut client = test_client().eliciting().with_session().connect().await;
    drop(client.elicit().await);

    let echo: serde_json::Value = serde_json::from_str(&client.next_message().await).unwrap();
    assert_eq!(echo, json!({"action": "cancel"}));
}

#[wasm_bindgen_test]
async fn an_elicitation_request_event_serializes_its_request() {
    let mut client = test_client().eliciting().with_session().connect().await;
    client.prompt().await;
    client.next_update().await;

    let event = client.next_event().await;
    let json: serde_json::Value = serde_json::from_str(&String::from(JSON::stringify(&event.0).unwrap())).unwrap();
    assert_eq!(json["type"], "elicitation_request");
    assert_eq!(json["elicitation"]["message"], "Continue?");
}

#[wasm_bindgen_test]
async fn a_prompt_reduces_into_conversation_snapshots() {
    let mut client = test_client().with_session().connect().await;
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
    assert_eq!(settled.json()["activity"], "idle");
}

#[wasm_bindgen_test]
async fn a_prompt_during_a_turn_rejects_with_turn_in_progress() {
    let client = test_client().eliciting().with_session().connect().await;
    client.prompt().await;
    let error = client.try_prompt().await.expect_err("the elicitation holds the turn open");
    assert_eq!(code(&error), "turn_in_progress");
}

#[wasm_bindgen_test]
async fn a_new_session_replaces_the_current_conversation() {
    let mut client = test_client().after_turn().connect().await;

    client.new_session().await.expect("session is created");

    assert_eq!(client.next_conversation().await.json()["items"], json!([]));
}

#[wasm_bindgen_test]
async fn resuming_with_replay_rebuilds_the_conversation() {
    let mut client = test_client().after_turn().connect().await;

    client.resume_session("fake-session").await.expect("session resumes");

    let restarted = client.next_conversation().await;
    assert_eq!(restarted.json()["items"], json!([]), "resuming starts the conversation over");
    let current = client.latest_conversation().json();
    assert_eq!(current["turn"], "idle");
    assert_eq!(current["items"].as_array().unwrap().len(), 1);
    assert_eq!(current["items"][0]["content"], json!([{"type": "text", "text": "saved answer"}]));
}

#[wasm_bindgen_test]
async fn updates_for_another_session_do_not_reach_the_current_conversation() {
    let mut client = test_client().connect().await;
    client.resume_session("other-session").await.expect("session resumes");

    let current = client.latest_conversation().json();
    assert_eq!(current["items"], json!([]), "the replayed fake-session message is not the current session's");
}

#[wasm_bindgen_test]
async fn a_failed_resume_leaves_no_current_session() {
    let mut client = test_client().closing().connect().await;
    client.resume_session("fake-session").await.expect_err("the server closes instead of answering");
    assert_eq!(client.latest_conversation().json(), serde_json::Value::Null);
    let error = client.try_prompt().await.expect_err("no session is current");
    assert_eq!(code(&error), "no_session");
}

#[wasm_bindgen_test]
async fn closing_the_session_stops_tracking_it() {
    let mut client = test_client().with_session().connect().await;

    client.close_session().await.expect("session closes");

    assert_eq!(client.latest_conversation().json(), serde_json::Value::Null);
    let error = client.try_prompt().await.expect_err("no session is current");
    assert_eq!(code(&error), "no_session");
}

#[wasm_bindgen_test]
async fn malformed_prompt_rejects_with_invalid_argument() {
    let client = test_client().with_session().connect().await;
    let error = resolve::<PromptResponse>(client.aether.prompt(js(&json!("not a prompt")))).await;
    assert_eq!(code(&error.expect_err("malformed prompt rejects")), "invalid_argument");
}

#[wasm_bindgen_test]
async fn disconnect_emits_connection_closed_and_later_requests_reject() {
    let mut client = test_client().with_session().connect().await;
    JsFuture::from(client.aether.disconnect()).await.expect("disconnect resolves");
    let event = client.emitted_event();
    assert_eq!(event.kind(), "connection_closed");
    assert!(event.get("close").is_null(), "this client closed the socket");

    let error = client.new_session().await.expect_err("requests after close reject");
    assert_eq!(code(&error), "protocol");
    let error = client.cancel().await.expect_err("notifications after close reject");
    assert_eq!(code(&error), "protocol");
}

#[wasm_bindgen_test]
async fn releasing_the_client_closes_the_connection() {
    let TestClient { aether, mut events, _on_event, .. } = test_client().connect().await;
    drop(aether);

    let event = Event(events.next().await.expect("event stream ended"));
    assert_eq!(event.kind(), "connection_closed");
    assert!(event.get("close").is_null(), "this client closed the socket");
}

#[wasm_bindgen_test]
async fn server_close_emits_connection_closed_with_its_code_and_reason() {
    let mut client = test_client().closing().connect().await;
    client.new_session().await.expect_err("the server closes instead of answering");
    let event = client.next_event().await;
    assert_eq!(event.kind(), "connection_closed");
    assert_eq!(
        from_js::<serde_json::Value>(event.get("close")),
        json!({"code": 4000, "reason": "closed by the fake agent"})
    );
}

#[wasm_bindgen_test]
async fn connect_failure_rejects_with_connect_failed_and_the_abnormal_close() {
    let error = test_client().url("ws://127.0.0.1:1").connect_error().await;
    assert_eq!(code(&error), "connect_failed");
    assert_eq!(close(&error)["code"], 1006);
}

#[wasm_bindgen_test]
async fn malformed_url_rejects_with_connect_failed() {
    let error = test_client().url("not a websocket url").connect_error().await;
    assert_eq!(code(&error), "connect_failed");
}

#[wasm_bindgen_test]
async fn close_before_initialize_rejects_with_the_servers_close() {
    let error = test_client().another_client_attached().connect_error().await;
    assert_eq!(code(&error), "connect_failed");
    assert_eq!(close(&error), json!({"code": 4409, "reason": "another client is attached"}));
}

#[wasm_bindgen_test]
async fn binary_frame_fails_the_connection() {
    test_client().sending_binary().connect_error().await;
}

#[wasm_bindgen_test]
async fn protocols_option_offers_websocket_subprotocols() {
    let client = test_client().requiring_protocol().options(&json!({"protocols": ["fake-agent.v1"]})).connect().await;
    let response: InitializeResponse = from_js(client.aether.initialize_response());
    assert!(response.meta.is_some(), "the gateway accepted the subprotocol and the agent initialized");
}

#[wasm_bindgen_test]
async fn handshake_rejected_for_a_missing_protocol_rejects_with_an_abnormal_close() {
    let error = test_client().requiring_protocol().connect_error().await;
    assert_eq!(code(&error), "connect_failed");
    assert_eq!(close(&error)["code"], 1006);
}

#[wasm_bindgen_test]
async fn malformed_options_reject_with_invalid_argument() {
    let error = test_client().options(&json!({"protocols": "fake-agent.v1"})).connect_error().await;
    assert_eq!(code(&error), "invalid_argument");
}

/// A client of the fake agent's basic scenario, which replies to every prompt.
fn test_client() -> TestClientBuilder {
    TestClientBuilder { url: common::url("/basic"), options: JsValue::UNDEFINED, session: false, turn: false }
}

struct TestClientBuilder {
    url: String,
    options: JsValue,
    session: bool,
    turn: bool,
}

struct TestClient {
    aether: AetherClient,
    events: mpsc::UnboundedReceiver<JsValue>,
    conversations: mpsc::UnboundedReceiver<JsValue>,
    _on_event: Closure<dyn FnMut(JsValue)>,
}

impl TestClientBuilder {
    /// The agent's turn asks "Continue?", then replies with the answer as JSON.
    fn eliciting(self) -> Self {
        self.scenario("/elicitation")
    }

    /// The agent answers `initialize`, then closes the socket with its own code and reason instead of answering.
    fn closing(self) -> Self {
        self.scenario("/close")
    }

    /// The gateway closes the socket before `initialize`, as it does while another client is attached.
    fn another_client_attached(self) -> Self {
        self.scenario("/attached")
    }

    fn sending_binary(self) -> Self {
        self.scenario("/binary")
    }

    /// The gateway rejects the handshake unless the `fake-agent.v1` subprotocol is offered.
    fn requiring_protocol(self) -> Self {
        self.scenario("/protocol")
    }

    fn url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    fn options(mut self, options: &serde_json::Value) -> Self {
        self.options = js(options);
        self
    }

    /// Start with a current session, its empty conversation already taken.
    fn with_session(mut self) -> Self {
        self.session = true;
        self
    }

    /// Start with a session whose first turn has gone idle.
    fn after_turn(mut self) -> Self {
        self.turn = true;
        self.with_session()
    }

    async fn connect(self) -> TestClient {
        let (events_tx, events) = mpsc::unbounded();
        let (conversations_tx, conversations) = mpsc::unbounded();
        let on_event = Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let channel =
                if Event(event.clone()).kind() == "conversation_changed" { &conversations_tx } else { &events_tx };
            let _ = channel.unbounded_send(event);
        });
        let aether =
            AetherClient::connect(self.url, on_event.as_ref().unchecked_ref::<Function>().clone(), self.options)
                .await
                .expect("connects to the fake agent");
        let mut client = TestClient { aether, events, conversations, _on_event: on_event };
        if self.session {
            client.new_session().await.expect("session is created");
            client.latest_conversation();
        }
        if self.turn {
            client.prompt().await;
            client.conversation_where(|conversation| conversation["turn"] == "idle").await;
        }
        client
    }

    async fn connect_error(self) -> JsValue {
        AetherClient::connect(self.url, Function::new_no_args(""), self.options)
            .await
            .err()
            .expect("connect fails")
            .into()
    }

    fn scenario(self, path: &str) -> Self {
        self.url(common::url(path))
    }
}

impl TestClient {
    async fn new_session(&self) -> Result<NewSessionResponse, JsValue> {
        resolve(self.aether.new_session(js(&json!({"cwd": "/workspace"})))).await
    }

    async fn resume_session(&self, session_id: &str) -> Result<serde_json::Value, JsValue> {
        let request = js(&json!({"sessionId": session_id, "cwd": "/workspace", "replayFrom": {"type": "start"}}));
        resolve(self.aether.resume_session(request)).await
    }

    async fn prompt(&self) -> PromptResponse {
        self.try_prompt().await.expect("prompt is accepted")
    }

    async fn try_prompt(&self) -> Result<PromptResponse, JsValue> {
        resolve(self.aether.prompt(prompt_content())).await
    }

    async fn cancel(&self) -> Result<(), JsValue> {
        resolve(self.aether.cancel()).await
    }

    async fn close_session(&self) -> Result<serde_json::Value, JsValue> {
        resolve(self.aether.close_session()).await
    }

    /// Prompt the current session, for the eliciting agent's turn to ask its question.
    async fn elicit(&mut self) -> Elicitation {
        self.prompt().await;
        assert!(matches!(self.next_update().await, SessionUpdate::StateUpdate(StateUpdate::Running(_))));
        let event = self.next_event().await;
        assert_eq!(event.kind(), "elicitation_request");
        Elicitation::try_from_js_value(event.get("elicitation")).expect("the event carries an Elicitation")
    }

    async fn next_event(&mut self) -> Event {
        Event(self.events.next().await.expect("event stream ended"))
    }

    /// The next event, which must already have been emitted.
    fn emitted_event(&mut self) -> Event {
        Event(self.events.try_recv().expect("an event was emitted"))
    }

    /// The next `conversation_changed` snapshot.
    async fn next_conversation(&mut self) -> Snapshot {
        Snapshot(Event(self.conversations.next().await.expect("event stream ended")).get("conversation"))
    }

    /// The last `conversation_changed` snapshot emitted so far, which must follow the last one taken.
    fn latest_conversation(&mut self) -> Snapshot {
        let latest = iter::from_fn(|| self.conversations.try_recv().ok()).last();
        Snapshot(Event(latest.expect("a conversation was emitted")).get("conversation"))
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
}

async fn resolve<T: DeserializeOwned>(promise: Promise) -> Result<T, JsValue> {
    JsFuture::from(promise).await.map(from_js)
}

fn prompt_content() -> JsValue {
    js(&json!([{"type": "text", "text": "hi"}]))
}

fn js(value: &serde_json::Value) -> JsValue {
    JSON::parse(&value.to_string()).unwrap()
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> T {
    serde_wasm_bindgen::from_value(value).unwrap()
}

fn code(error: &JsValue) -> String {
    Reflect::get(error, &"code".into()).unwrap().as_string().unwrap()
}

fn close(error: &JsValue) -> serde_json::Value {
    from_js(Reflect::get(error, &"close".into()).unwrap())
}
