use super::provider::ModelTarget;
use super::sdk::{self, request_error};
use crate::providers::anthropic::mappers::build_request;
use crate::providers::anthropic::streaming::decode_messages;
use crate::providers::anthropic::types::Request;
use crate::providers::response_stream::StreamAssembler;
use crate::{Context, LlmResponseStream, Result};
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::primitives::Blob;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::ResponseStream;
use aws_sdk_bedrockruntime::types::error::ResponseStreamError;
use serde::Serialize;
use std::time::Duration;
use tracing::{info, warn};

/// Stream a response from `InvokeModelWithResponseStream`.
pub(super) fn stream(
    client: &Client,
    target: ModelTarget,
    context: Context,
    idle_timeout: Duration,
) -> LlmResponseStream {
    let client = client.clone();
    let mut decode_payload = decode_messages::<Blob>();
    sdk::stream(
        async move { send(&client, &target, &context).await },
        move |event, turn: &mut StreamAssembler<u32>| match event {
            ResponseStream::Chunk(part) => {
                part.bytes.map_or_else(|| Ok(Vec::new()), |bytes| decode_payload(bytes, turn))
            }
            other => {
                warn!("Unhandled Bedrock invoke stream event: {other:?}");
                Ok(Vec::new())
            }
        },
        idle_timeout,
    )
}

#[derive(Debug, Serialize)]
struct InvokeRequest {
    anthropic_version: &'static str,
    #[serde(flatten)]
    request: Request,
}

async fn send(
    client: &Client,
    target: &ModelTarget,
    context: &Context,
) -> Result<EventReceiver<ResponseStream, ResponseStreamError>> {
    let body = InvokeRequest {
        anthropic_version: "bedrock-2023-05-31",
        request: build_request(target.identity.as_ref(), context)?,
    };

    info!(
        model = %target.model,
        inference_profile_arn = ?target.inference_profile_arn,
        "Sending Bedrock invoke_model_with_response_stream request"
    );

    let response = client
        .invoke_model_with_response_stream()
        .model_id(target.request_id())
        .content_type("application/json")
        .accept("application/json")
        .body(Blob::new(serde_json::to_vec(&body)?))
        .send()
        .await
        .map_err(|e| request_error(target, &e))?;

    Ok(response.body)
}
