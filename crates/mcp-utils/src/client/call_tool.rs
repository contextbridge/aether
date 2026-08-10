use rmcp::{
    RoleClient, Service,
    model::{CallToolRequestParams, CallToolResponse, ClientRequest, ProgressToken, Request, ServerResult},
    service::{PeerRequestOptions, RequestHandle, RunningService, ServiceError},
};

pub struct CallToolRequestHandle {
    inner: RequestHandle<RoleClient>,
}

pub async fn call_tool_with_options<T: Service<RoleClient>>(
    client: &RunningService<RoleClient, T>,
    params: CallToolRequestParams,
    options: PeerRequestOptions,
) -> Result<CallToolRequestHandle, ServiceError> {
    let inner = client.send_cancellable_request(ClientRequest::CallToolRequest(Request::new(params)), options).await?;
    Ok(CallToolRequestHandle { inner })
}

impl CallToolRequestHandle {
    pub fn progress_token(&self) -> &ProgressToken {
        &self.inner.progress_token
    }

    pub async fn await_response(self) -> Result<CallToolResponse, ServiceError> {
        match self.inner.await_response().await? {
            ServerResult::CallToolResult(result) => Ok(CallToolResponse::Complete(result)),
            ServerResult::InputRequiredResult(result) => Ok(CallToolResponse::InputRequired(result)),
            ServerResult::CreateTaskResult(result) => Ok(CallToolResponse::Task(result)),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    pub async fn cancel(self, reason: Option<String>) -> Result<(), ServiceError> {
        self.inner.cancel(reason).await
    }
}
