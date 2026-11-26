use futures::StreamExt;
use reqwest::header::HeaderMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use super::streaming::{ResponseEvents, ResponsesStreamEvent, decode_event};
use super::types::ResponsesRequest;
use super::websocket::{SocketOutcome, SocketSession};
use crate::providers::http::{post_sse, responses_code};
use crate::providers::response_stream::{OpenedStream, response_stream};
use crate::{LlmResponseStream, ResponsesTransport, Result};

#[derive(Clone)]
pub(crate) struct ResponsesClient {
    http: reqwest::Client,
    endpoint: String,
    sockets: Option<Arc<SocketSession>>,
    idle_timeout: Duration,
}

pub(crate) struct ResponsesCall {
    pub request: ResponsesRequest,
    pub headers: HeaderMap,
}

impl ResponsesClient {
    pub(crate) fn new(
        http: reqwest::Client,
        base_url: &str,
        transport: ResponsesTransport,
        idle_timeout: Duration,
    ) -> Self {
        let sockets = match transport {
            ResponsesTransport::Http => None,
            ResponsesTransport::WebSocket => Some(Arc::default()),
        };
        Self { http, endpoint: format!("{base_url}/responses"), sockets, idle_timeout }
    }

    pub(crate) fn for_another_owner(mut self) -> Self {
        self.sockets = self.sockets.map(|sockets| Arc::new(sockets.for_another_owner()));
        self
    }

    pub(crate) fn stream(
        &self,
        call: impl Future<Output = Result<ResponsesCall>> + Send + 'static,
    ) -> LlmResponseStream {
        let client = self.clone();
        response_stream(async move { client.open(call.await?).await }, decode_event, self.idle_timeout)
    }

    async fn open(&self, call: ResponsesCall) -> Result<OpenedStream<ResponseEvents>> {
        let call = match self.sockets.as_ref().filter(|sockets| sockets.accepts_upgrades()) {
            Some(sockets) => match sockets.open(&self.endpoint, call).await? {
                SocketOutcome::Stream(events) => return Ok(OpenedStream::new(events)),
                SocketOutcome::UpgradeRefused(call) => *call,
            },
            None => call,
        };

        let opened = post_sse(&self.http, &self.endpoint, call.headers, &call.request, responses_code).await?;
        let mut started = false;
        Ok(opened.map_events(|data| {
            Box::pin(data.map(move |data| ResponsesStreamEvent::parse(&data?, &mut started))) as ResponseEvents
        }))
    }
}

#[cfg(test)]
mod tests {
    use crate::providers::generic::{GenericProvider, OPENAI};
    use crate::providers::test_capture_server::{CaptureServer, CapturedRequest, ProviderFixture, SocketBehavior};
    use crate::testing::{assistant_message, tool_result};
    use crate::{
        AssistantReply, ChatMessage, Context, LlmError, LlmResponse, MessageId, ProviderAuthMode,
        ProviderConnectionConfig, ProviderErrorKind, ReasoningEffort, ResponsesTransport, Result,
        StreamingModelProvider, ToolDefinition,
    };
    use futures::StreamExt;
    use http::StatusCode;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn the_first_turn_sends_the_whole_conversation_in_one_frame() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_eq!(request.socket, Some(0));
        assert_eq!(request.headers["OpenAI-Beta"], "responses_websockets=2026-02-06");
        assert_eq!(request.body["type"], "response.create");
        assert_eq!(request.body["model"], "gpt-5.5");
        assert_eq!(request.body["store"], false);
        assert_eq!(roles(&request), ["developer", "user"]);
        assert!(request.body.get("previous_response_id").is_none());
        assert!(request.body.get("stream").is_none());
    }

    #[tokio::test]
    async fn every_follow_up_turn_sends_only_what_is_new() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.turn().await;

        for turn in 1..4 {
            conversation.say("And again?");
            let [request] = conversation.turn().await.try_into().unwrap();

            assert_eq!(request.socket, Some(0));
            assert_eq!(request.body["previous_response_id"], format!("resp_0_{}", turn - 1));
            assert_eq!(roles(&request), ["user"]);
            assert_eq!(request.body["input"][0]["content"][0]["text"], "And again?");
        }
    }

    #[tokio::test]
    async fn tool_results_continue_the_response_that_requested_them() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::CallsTool).await;
        conversation.send_and_discard_reply().await;
        conversation.context.add_message(assistant_message().tool_call("call_1", "read", "{}").build());
        conversation.context.add_message(tool_result("call_1", "read", "{}", "contents of a.rs"));

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_eq!(request.body["previous_response_id"], "resp_0_0");
        assert_eq!(
            request.body["input"],
            json!([{ "type": "function_call_output", "call_id": "call_1", "output": "contents of a.rs" }])
        );
    }

    #[tokio::test]
    async fn a_reply_the_server_split_into_messages_is_still_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::SplitsReply).await;
        conversation.turn().await;
        conversation.say("And again?");

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_eq!(request.body["previous_response_id"], "resp_0_0");
        assert_eq!(roles(&request), ["user"]);
    }

    #[tokio::test]
    async fn a_reply_that_differs_from_what_the_server_sent_is_not_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.send_and_discard_reply().await;
        conversation.context.add_message(assistant_message().text("not what the server said").build());
        conversation.say("And again?");

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_sent_whole_conversation(&request, &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn a_reply_dropped_from_history_is_not_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.send_and_discard_reply().await;
        conversation.say("And again?");

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_sent_whole_conversation(&request, &["developer", "user", "user"]);
    }

    #[tokio::test]
    async fn output_history_cannot_represent_is_not_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::UnknownOutput).await;
        conversation.turn().await;
        conversation.say("And again?");

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_sent_whole_conversation(&request, &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn changed_reasoning_effort_is_not_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.turn().await;
        conversation.say("Think harder");
        conversation.context.set_reasoning_effort(ReasoningEffort::High);

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_sent_whole_conversation(&request, &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn changed_tools_are_not_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.turn().await;
        conversation.say("Use the tool");
        conversation.context.set_tools(vec![ToolDefinition::new("read", "Reads a file", json!({}))]);

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_sent_whole_conversation(&request, &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn rewritten_history_is_not_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.turn().await;
        conversation.context.replace_conversation(vec![ChatMessage::user("A summary of what came before")]);

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_sent_whole_conversation(&request, &["developer", "user"]);
    }

    #[tokio::test]
    async fn a_turn_that_cannot_continue_opens_a_socket_of_its_own() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        conversation.turn().await;
        conversation.context.replace_conversation(vec![ChatMessage::user("A summary of what came before")]);

        let [rewritten] = conversation.turn().await.try_into().unwrap();
        conversation.say("And again?");
        let [follow_up] = conversation.turn().await.try_into().unwrap();

        assert_eq!(rewritten.socket, Some(1));
        assert_eq!(follow_up.socket, Some(1));
        assert_eq!(follow_up.body["previous_response_id"], "resp_1_0");
    }

    #[tokio::test]
    async fn agents_sharing_a_session_each_keep_their_own_socket() {
        let mut parent = Conversation::over_sockets(SocketBehavior::Remembers).await;
        let child_provider = parent.fixture.provider.clone().with_model("gpt-5.5");
        let mut child_context = Context::new(vec![ChatMessage::user("Explore the repository")], vec![]);
        child_context.set_session_affinity_key(parent.context.session_affinity_key().map(str::to_owned));
        parent.turn().await;
        take_turn(&child_provider, &mut child_context, &mut parent.fixture.server).await;

        for turn in 1..4 {
            parent.say("And again?");
            child_context.add_message(ChatMessage::user("Keep going"));
            let [from_parent] = parent.turn().await.try_into().unwrap();
            let [from_child] =
                take_turn(&child_provider, &mut child_context, &mut parent.fixture.server).await.try_into().unwrap();

            assert_eq!(from_parent.socket, Some(0));
            assert_eq!(from_parent.body["previous_response_id"], format!("resp_0_{}", turn - 1));
            assert_eq!(from_child.socket, Some(1));
            assert_eq!(from_child.body["previous_response_id"], format!("resp_1_{}", turn - 1));
        }
    }

    #[tokio::test]
    async fn a_socket_that_is_mid_response_is_not_shared() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        let mut unfinished = conversation.fixture.provider.stream_response(&conversation.context);
        unfinished.next().await.unwrap().unwrap();
        let active = conversation.fixture.server.captured().await;

        let [parallel] = conversation.turn().await.try_into().unwrap();

        assert_eq!((active.socket, parallel.socket), (Some(0), Some(1)));
    }

    #[tokio::test]
    async fn an_abandoned_response_is_never_continued() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Remembers).await;
        let mut abandoned = conversation.fixture.provider.stream_response(&conversation.context);
        abandoned.next().await.unwrap().unwrap();
        conversation.fixture.server.captured().await;
        drop(abandoned);
        conversation.say("Never mind");

        let [request] = conversation.turn().await.try_into().unwrap();

        assert_eq!(request.socket, Some(1));
        assert_sent_whole_conversation(&request, &["developer", "user", "user"]);
    }

    #[tokio::test]
    async fn a_response_the_server_forgot_is_replaced_by_the_whole_conversation() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Forgets).await;
        conversation.turn().await;
        conversation.say("And again?");

        let [continued, restarted] = conversation.turn().await.try_into().unwrap();

        assert_eq!(continued.body["previous_response_id"], "resp_0_0");
        assert_eq!(restarted.socket, Some(1));
        assert_sent_whole_conversation(&restarted, &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn a_socket_past_its_time_limit_is_replaced() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::Expires).await;
        conversation.turn().await;
        conversation.say("And again?");

        let requests = conversation.turn().await;

        assert_eq!(sockets(&requests), [Some(0), Some(1)]);
        assert_sent_whole_conversation(&requests[1], &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn a_socket_the_server_closed_while_idle_is_replaced() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::ClosesWhenIdle).await;
        conversation.turn().await;
        conversation.say("And again?");

        let requests = conversation.turn().await;

        let restarted = requests.last().unwrap();
        assert_eq!(restarted.socket, Some(1));
        assert_sent_whole_conversation(restarted, &["developer", "user", "assistant", "user"]);
    }

    #[tokio::test]
    async fn a_socket_that_closes_mid_response_fails_the_turn_so_it_can_be_retried() {
        let mut conversation = Conversation::over_sockets(SocketBehavior::DropsMidResponse).await;

        let responses = conversation.send().await;

        let error = responses.last().and_then(|response| response.as_ref().err()).and_then(LlmError::provider);
        assert_eq!(error.map(|error| error.kind), Some(ProviderErrorKind::StreamInterrupted), "{responses:?}");
        assert_eq!(conversation.fixture.server.upgrades.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn an_endpoint_without_websockets_is_served_over_http_from_then_on() {
        let mut conversation =
            Conversation::over_sockets(SocketBehavior::RejectsUpgrade(StatusCode::UPGRADE_REQUIRED)).await;

        let first = conversation.turn().await;
        conversation.say("And again?");
        let second = conversation.turn().await;

        assert_eq!(sockets(&first), [None]);
        assert_eq!(first[0].body["stream"], true);
        assert!(first[0].body.get("type").is_none());
        assert_eq!(sockets(&second), [None]);
        assert_eq!(roles(&second[0]), ["developer", "user", "assistant", "user"]);
        assert_eq!(conversation.fixture.server.upgrades.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_rejected_handshake_is_an_error_not_a_reason_to_fall_back() {
        let mut conversation =
            Conversation::over_sockets(SocketBehavior::RejectsUpgrade(StatusCode::UNAUTHORIZED)).await;

        let responses = conversation.send().await;

        let error = responses.last().and_then(|response| response.as_ref().err()).and_then(LlmError::provider);
        assert_eq!(error.map(|error| error.kind), Some(ProviderErrorKind::Authentication), "{responses:?}");
        assert_eq!(error.and_then(|error| error.http_status), Some(401));
        assert!(conversation.fixture.server.drain().is_empty());
    }

    #[tokio::test]
    async fn a_custom_url_is_served_over_http_unless_websockets_are_requested() {
        let fixture = ProviderFixture::builder(&OPENAI).model("gpt-5.5").build().await;
        let mut conversation = Conversation::new(fixture);

        let requests = conversation.turn().await;

        assert_eq!(sockets(&requests), [None]);
    }

    struct Conversation {
        fixture: ProviderFixture,
        context: Context,
    }

    impl Conversation {
        async fn over_sockets(sockets: SocketBehavior) -> Self {
            let connection = ProviderConnectionConfig {
                auth_mode: ProviderAuthMode::None,
                transport: Some(ResponsesTransport::WebSocket),
                ..Default::default()
            };
            Self::new(
                ProviderFixture::builder(&OPENAI)
                    .model("gpt-5.5")
                    .connection(connection)
                    .sockets(sockets)
                    .build()
                    .await,
            )
        }

        fn new(fixture: ProviderFixture) -> Self {
            let mut context = Context::new(vec![ChatMessage::system("Be brief"), ChatMessage::user("Hello")], vec![]);
            context.set_session_affinity_key(Some("main".into()));
            Self { fixture, context }
        }

        fn say(&mut self, text: &str) {
            self.context.add_message(ChatMessage::user(text));
        }

        async fn turn(&mut self) -> Vec<CapturedRequest> {
            take_turn(&self.fixture.provider, &mut self.context, &mut self.fixture.server).await
        }

        async fn send_and_discard_reply(&mut self) {
            assert_completed(&self.send().await);
            self.fixture.server.drain();
        }

        async fn send(&mut self) -> Vec<Result<LlmResponse>> {
            self.fixture.provider.stream_response(&self.context).collect().await
        }
    }

    async fn take_turn(
        provider: &GenericProvider,
        context: &mut Context,
        server: &mut CaptureServer,
    ) -> Vec<CapturedRequest> {
        let responses = provider.stream_response(context).collect::<Vec<_>>().await;
        assert_completed(&responses);

        let mut reply = AssistantReply::default();
        for response in responses {
            reply.push(&response.unwrap());
        }
        context.add_message(ChatMessage::assistant_reply(MessageId::new(), None, reply, &[]));
        server.drain()
    }

    fn assert_completed(responses: &[Result<LlmResponse>]) {
        assert!(matches!(responses.last(), Some(Ok(LlmResponse::Done { .. }))), "{responses:?}");
    }

    fn assert_sent_whole_conversation(request: &CapturedRequest, expected_roles: &[&str]) {
        assert!(request.body.get("previous_response_id").is_none(), "{:?}", request.body);
        assert_eq!(roles(request), expected_roles);
    }

    fn roles(request: &CapturedRequest) -> Vec<&str> {
        request.body["input"].as_array().unwrap().iter().map(|item| item["role"].as_str().unwrap()).collect()
    }

    fn sockets(requests: &[CapturedRequest]) -> Vec<Option<usize>> {
        requests.iter().map(|request| request.socket).collect()
    }
}
