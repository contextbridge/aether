use super::McpClient;
use super::task::{TaskErrorReason, cancel_server_task, stream_task_events};
use async_stream::stream;
use futures::future::Either;
use futures::{Stream, StreamExt, stream};
use rmcp::RoleClient;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ClientRequest, CreateTaskResult, DEFAULT_MRTR_MAX_ROUNDS,
    InputRequests, InputRequiredResult, ProgressNotificationParam, Request, RequestMetaObject, ServerResult, Task,
};
use rmcp::service::{PeerRequestOptions, RequestHandle, ServiceError};
use std::future::pending;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use std::time::Duration;
use thiserror::Error;
use tokio::time::{Instant, sleep, sleep_until};
use tokio_util::sync::CancellationToken;

pub struct ToolCall {
    events: Pin<Box<dyn Stream<Item = ToolCallEvent> + Send>>,
}

#[derive(Debug)]
pub enum ToolCallEvent {
    Progress(ProgressNotificationParam),
    TaskCreated(CreateTaskResult),
    TaskStatus(Task),
    Done(Result<CallToolResult, ToolCallError>),
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallOptions {
    pub timeout: Option<Duration>,
    pub meta: Option<RequestMetaObject>,
    pub cancel: CancellationToken,
}

#[derive(Debug, Error)]
pub enum ToolCallError {
    #[error("Failed to send tool request: {0}")]
    Send(#[source] ServiceError),
    #[error("Tool execution failed: {0}")]
    Call(#[source] ServiceError),
    #[error("Tool call exceeded its {0:?} timeout")]
    TimedOut(Duration),
    #[error("Server requested an input kind this client does not support (sampling or roots)")]
    UnsupportedInput,
    #[error("Server requested input without any input requests or request state")]
    EmptyInputRequired,
    #[error("Server requested input again after the user cancelled")]
    RePromptAfterCancel,
    #[error("Server did not complete within {DEFAULT_MRTR_MAX_ROUNDS} MRTR input rounds")]
    InputRoundsExceeded,
    #[error("Task '{task_id}' {reason}")]
    Task {
        task_id: String,
        #[source]
        reason: Box<TaskErrorReason>,
    },
    #[error("Server returned a tool call response kind this client does not support")]
    UnsupportedResponse,
    #[error("Tool call was cancelled")]
    Cancelled,
}

impl ToolCallOptions {
    pub fn with_timeout(timeout: Duration) -> Self {
        Self { timeout: Some(timeout), ..Self::default() }
    }
}

impl ToolCall {
    pub(crate) fn new(client: McpClient, params: CallToolRequestParams, options: ToolCallOptions) -> Self {
        let ToolCallOptions { timeout, meta, cancel } = options;
        let events = stream_events(client.clone(), params, meta);
        Self { events: Box::pin(bounded(client, events, timeout, cancel)) }
    }

    pub async fn result(self) -> Result<CallToolResult, ToolCallError> {
        self.result_with_progress(|_| std::future::ready(())).await
    }

    pub async fn result_with_progress<F: Future<Output = ()>>(
        mut self,
        mut on_progress: impl FnMut(ProgressNotificationParam) -> F,
    ) -> Result<CallToolResult, ToolCallError> {
        while let Some(event) = self.next().await {
            match event {
                ToolCallEvent::Progress(progress) => on_progress(progress).await,
                ToolCallEvent::Done(result) => return result,
                ToolCallEvent::TaskCreated(_) | ToolCallEvent::TaskStatus(_) => {}
            }
        }
        Err(ToolCallError::Cancelled)
    }
}

impl Stream for ToolCall {
    type Item = ToolCallEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.events.as_mut().poll_next(cx)
    }
}

impl From<ToolCallError> for ToolCallEvent {
    fn from(error: ToolCallError) -> Self {
        Self::Done(Err(error))
    }
}

pub(super) fn with_progress<'a, T: 'a>(
    progress: impl Stream<Item = ProgressNotificationParam> + 'a,
    events: impl Stream<Item = T> + 'a,
) -> impl Stream<Item = Either<ProgressNotificationParam, T>> + 'a {
    stream! {
        let mut progress = pin!(progress);
        let mut events = pin!(events);
        loop {
            tokio::select! {
                biased;
                progress_event = progress.next() => match progress_event {
                    Some(progress_event) => yield Either::Left(progress_event),
                    None => {
                        while let Some(event) = events.next().await {
                            yield Either::Right(event);
                        }
                        return;
                    }
                },
                event = events.next() => match event {
                    Some(event) => yield Either::Right(event),
                    None => return,
                },
            }
        }
    }
}

fn bounded(
    client: McpClient,
    events: impl Stream<Item = ToolCallEvent> + Send + 'static,
    timeout: Option<Duration>,
    cancel: CancellationToken,
) -> impl Stream<Item = ToolCallEvent> + Send + 'static {
    stream! {
        let mut events = Box::pin(events);
        let mut expired = pin!(expiry(timeout));
        let mut cancelled = pin!(cancel.cancelled());
        let mut task = None;

        let interrupted = loop {
            tokio::select! {
                biased;
                event = events.next() => {
                    let Some(event) = event else { return };
                    match &event {
                        ToolCallEvent::TaskCreated(created) => task = Some(created.task.clone()),
                        ToolCallEvent::TaskStatus(status) => task = Some(status.clone()),
                        ToolCallEvent::Progress(_) | ToolCallEvent::Done(_) => {}
                    }
                    let done = matches!(event, ToolCallEvent::Done(_));
                    yield event;
                    if done {
                        return;
                    }
                }
                timeout = &mut expired => break ToolCallError::TimedOut(timeout),
                () = &mut cancelled => break ToolCallError::Cancelled,
            }
        };

        drop(events);
        if let Some(task) = &task {
            cancel_server_task(&client, &task.task_id).await;
        }
        yield interrupted.into();
    }
}

fn stream_events(
    client: McpClient,
    mut params: CallToolRequestParams,
    meta: Option<RequestMetaObject>,
) -> impl Stream<Item = ToolCallEvent> + Send + 'static {
    stream! {
        let mut mrtr_state = MrtrState::new();

        loop {
            let request = ClientRequest::CallToolRequest(Request::new(params.clone()));
            let handle = match client.peer.send_cancellable_request(request, peer_request_options(meta.clone())).await {
                Ok(handle) => handle,
                Err(error) => {
                    yield ToolCallError::Send(error).into();
                    return;
                }
            };

            let mut progress = client.handler.progress.subscribe(handle.progress_token.clone()).await;
            let mut response = None;
            {
                let mut awaited = pin!(with_progress(&mut progress, stream::once(await_tool_response(handle))));
                while let Some(event) = awaited.next().await {
                    match event {
                        Either::Left(progress_event) => yield ToolCallEvent::Progress(progress_event),
                        Either::Right(awaited_response) => response = Some(awaited_response),
                    }
                }
            }

            match response.unwrap_or(Err(ServiceError::UnexpectedResponse)) {
                Ok(CallToolResponse::Complete(result)) => {
                    yield ToolCallEvent::Done(Ok(result));
                    return;
                }
                Ok(CallToolResponse::InputRequired(input_required)) => match mrtr_state.tick(input_required) {
                    MrtrAction::Poll { backoff, request_state } => {
                        sleep(backoff).await;
                        params.input_responses = None;
                        params.request_state = Some(request_state);
                    }
                    MrtrAction::Elicit { input_requests, request_state } => {
                        let (responses, cancelled) = match client.handler.elicit_inputs(input_requests).await {
                            Ok(answered) => answered,
                            Err(error) => {
                                yield error.into();
                                return;
                            }
                        };
                        mrtr_state.record_cancelled(cancelled);
                        params.input_responses = Some(responses);
                        params.request_state = request_state;
                    }
                    MrtrAction::Abort(error) => {
                        yield error.into();
                        return;
                    }
                },
                Ok(CallToolResponse::Task(created)) => {
                    let mut events = pin!(stream_task_events(&client, created, progress));
                    while let Some(event) = events.next().await {
                        yield event;
                    }
                    return;
                }
                Ok(_) => {
                    yield ToolCallError::UnsupportedResponse.into();
                    return;
                }
                Err(error) => {
                    yield ToolCallError::Call(error).into();
                    return;
                }
            }
        }
    }
}

async fn expiry(timeout: Option<Duration>) -> Duration {
    match timeout.and_then(|timeout| Some((Instant::now().checked_add(timeout)?, timeout))) {
        Some((deadline, timeout)) => {
            sleep_until(deadline).await;
            timeout
        }
        None => pending().await,
    }
}

fn peer_request_options(meta: Option<RequestMetaObject>) -> PeerRequestOptions {
    meta.map_or_else(PeerRequestOptions::no_options, |meta| PeerRequestOptions::no_options().with_meta(meta))
}

async fn await_tool_response(handle: RequestHandle<RoleClient>) -> Result<CallToolResponse, ServiceError> {
    match handle.await_response().await? {
        ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
        ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
        ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
        _ => Err(ServiceError::UnexpectedResponse),
    }
}

struct MrtrState {
    input_request_rounds: usize,
    next_backoff: Duration,
    user_cancelled: bool,
}

enum MrtrAction {
    Poll { backoff: Duration, request_state: String },
    Elicit { input_requests: InputRequests, request_state: Option<String> },
    Abort(ToolCallError),
}

impl MrtrState {
    fn new() -> Self {
        Self { input_request_rounds: 0, next_backoff: BASE_BACKOFF, user_cancelled: false }
    }

    fn tick(&mut self, input_required: InputRequiredResult) -> MrtrAction {
        let input_requests = input_required.input_requests.filter(|requests| !requests.is_empty());
        match (input_requests, input_required.request_state) {
            (None, None) => MrtrAction::Abort(ToolCallError::EmptyInputRequired),
            (None, Some(request_state)) => {
                let backoff = self.next_backoff;
                self.next_backoff = (backoff * 2).min(MAX_BACKOFF);
                MrtrAction::Poll { backoff, request_state }
            }
            (Some(input_requests), request_state) => {
                if self.user_cancelled {
                    MrtrAction::Abort(ToolCallError::RePromptAfterCancel)
                } else if self.input_request_rounds == DEFAULT_MRTR_MAX_ROUNDS {
                    MrtrAction::Abort(ToolCallError::InputRoundsExceeded)
                } else {
                    self.input_request_rounds += 1;
                    self.next_backoff = BASE_BACKOFF;
                    MrtrAction::Elicit { input_requests, request_state }
                }
            }
        }
    }

    fn record_cancelled(&mut self, cancelled: bool) {
        self.user_cancelled |= cancelled;
    }
}

const BASE_BACKOFF: Duration = Duration::from_millis(50);
const MAX_BACKOFF: Duration = Duration::from_millis(1600);
