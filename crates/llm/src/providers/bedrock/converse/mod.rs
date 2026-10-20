mod mappers;
mod streaming;

use super::provider::ModelTarget;
use super::sdk::{self, request_error};
use crate::{Context, LlmModel, LlmResponseStream, Result};
use aws_sdk_bedrockruntime::Client;
use aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver;
use aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError;
use aws_sdk_bedrockruntime::types::{ConverseStreamOutput, InferenceConfiguration};
use mappers::{default_cache_point, map_messages, map_tools};
use std::time::Duration;
use streaming::decode_converse_event;
use tracing::info;

const DEFAULT_MAX_TOKENS: i32 = 16_384;

/// Stream a response from the Converse API, Bedrock's default transport.
pub(super) fn stream(
    client: &Client,
    target: ModelTarget,
    context: Context,
    idle_timeout: Duration,
) -> LlmResponseStream {
    let client = client.clone();
    sdk::stream(
        async move { send(&client, &target, &context).await },
        |event, turn| Ok(decode_converse_event(event, turn)),
        idle_timeout,
    )
}

async fn send(
    client: &Client,
    target: &ModelTarget,
    context: &Context,
) -> Result<EventReceiver<ConverseStreamOutput, ConverseStreamOutputError>> {
    let caching = target.identity.as_ref().is_some_and(LlmModel::supports_prompt_caching);
    let cache_point = caching.then(default_cache_point).transpose()?;
    let (system_blocks, messages) = map_messages(context.messages(), cache_point.as_ref())?;
    let settings = context.model_settings();
    let max_tokens = settings.max_tokens.and_then(|m| i32::try_from(m).ok()).unwrap_or(DEFAULT_MAX_TOKENS);
    let inference_config = InferenceConfiguration::builder()
        .max_tokens(max_tokens)
        .set_temperature(settings.temperature)
        .set_top_p(settings.top_p)
        .build();

    let tool_config =
        (!context.tools().is_empty()).then(|| map_tools(context.tools(), cache_point.as_ref())).transpose()?;

    info!(
        model = %target.model,
        inference_profile_arn = ?target.inference_profile_arn,
        "Sending Bedrock converse_stream request"
    );

    let response = client
        .converse_stream()
        .model_id(target.request_id())
        .set_messages(Some(messages))
        .set_system((!system_blocks.is_empty()).then_some(system_blocks))
        .inference_config(inference_config)
        .set_tool_config(tool_config)
        .send()
        .await
        .map_err(|e| request_error(target, &e))?;

    Ok(response.stream)
}
