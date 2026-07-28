// Upstream GenAI semantic conventions are still in development and marked
// deprecated until they're published in a stable crate. This module
// centralizes constants so other modules stay clean.
#![allow(deprecated)]

use opentelemetry::InstrumentationScope;
pub use opentelemetry_semantic_conventions::SCHEMA_URL as GENAI_SEMCONV_SCHEMA_URL;
use opentelemetry_semantic_conventions::{attribute, metric};

pub const ERROR_TYPE: &str = attribute::ERROR_TYPE;
pub const GEN_AI_INPUT_MESSAGES: &str = attribute::GEN_AI_INPUT_MESSAGES;
pub const GEN_AI_OPERATION_NAME: &str = attribute::GEN_AI_OPERATION_NAME;
pub const GEN_AI_OUTPUT_MESSAGES: &str = attribute::GEN_AI_OUTPUT_MESSAGES;
pub const GEN_AI_PROVIDER_NAME: &str = attribute::GEN_AI_PROVIDER_NAME;
pub const GEN_AI_REQUEST_MODEL: &str = attribute::GEN_AI_REQUEST_MODEL;
pub const GEN_AI_REQUEST_STREAM: &str = attribute::GEN_AI_REQUEST_STREAM;
pub const GEN_AI_RESPONSE_FINISH_REASONS: &str = attribute::GEN_AI_RESPONSE_FINISH_REASONS;
pub const GEN_AI_RESPONSE_TIME_TO_FIRST_CHUNK: &str = attribute::GEN_AI_RESPONSE_TIME_TO_FIRST_CHUNK;
pub const GEN_AI_TOKEN_TYPE: &str = attribute::GEN_AI_TOKEN_TYPE;
pub const GEN_AI_TOOL_CALL_ARGUMENTS: &str = attribute::GEN_AI_TOOL_CALL_ARGUMENTS;
pub const GEN_AI_TOOL_CALL_ID: &str = attribute::GEN_AI_TOOL_CALL_ID;
pub const GEN_AI_TOOL_CALL_RESULT: &str = attribute::GEN_AI_TOOL_CALL_RESULT;
pub const GEN_AI_TOOL_DEFINITIONS: &str = attribute::GEN_AI_TOOL_DEFINITIONS;
pub const GEN_AI_TOOL_NAME: &str = attribute::GEN_AI_TOOL_NAME;
pub const GEN_AI_USAGE_CACHE_CREATION_INPUT_TOKENS: &str = attribute::GEN_AI_USAGE_CACHE_CREATION_INPUT_TOKENS;
pub const GEN_AI_USAGE_CACHE_READ_INPUT_TOKENS: &str = attribute::GEN_AI_USAGE_CACHE_READ_INPUT_TOKENS;
pub const GEN_AI_USAGE_INPUT_TOKENS: &str = attribute::GEN_AI_USAGE_INPUT_TOKENS;
pub const GEN_AI_USAGE_OUTPUT_TOKENS: &str = attribute::GEN_AI_USAGE_OUTPUT_TOKENS;
pub const GEN_AI_USAGE_REASONING_OUTPUT_TOKENS: &str = attribute::GEN_AI_USAGE_REASONING_OUTPUT_TOKENS;
pub const SERVICE_VERSION: &str = attribute::SERVICE_VERSION;

pub const GEN_AI_CLIENT_OPERATION_DURATION: &str = metric::GEN_AI_CLIENT_OPERATION_DURATION;
pub const GEN_AI_CLIENT_OPERATION_TIME_PER_OUTPUT_CHUNK: &str = metric::GEN_AI_CLIENT_OPERATION_TIME_PER_OUTPUT_CHUNK;
pub const GEN_AI_CLIENT_OPERATION_TIME_TO_FIRST_CHUNK: &str = metric::GEN_AI_CLIENT_OPERATION_TIME_TO_FIRST_CHUNK;
pub const GEN_AI_CLIENT_TOKEN_USAGE: &str = metric::GEN_AI_CLIENT_TOKEN_USAGE;

pub const GENAI_RESPONSE_START_EVENT: &str = "gen_ai.response.start";
pub const GENAI_TOOL_CALL_START_EVENT: &str = "gen_ai.tool_call.start";
pub const MESSAGE_ID: &str = "message.id";
pub const TOOL_CALL_ID: &str = "tool_call.id";
pub const TOOL_CALL_NAME: &str = "tool_call.name";
pub const LLM_ATTEMPT: &str = "aether.llm.attempt";
pub const LLM_PURPOSE: &str = "aether.llm.purpose";
pub const AI_INPUT_TOKEN_PRICE: &str = "$ai_input_token_price";
pub const AI_OUTPUT_TOKEN_PRICE: &str = "$ai_output_token_price";
pub const AI_CACHE_READ_TOKEN_PRICE: &str = "$ai_cache_read_token_price";
pub const AI_CACHE_WRITE_TOKEN_PRICE: &str = "$ai_cache_write_token_price";
pub const AI_CACHE_REPORTING_EXCLUSIVE: &str = "$ai_cache_reporting_exclusive";
pub const AI_REASONING_TOKENS: &str = "$ai_reasoning_tokens";

pub fn genai_instrumentation_scope(version: impl Into<String>) -> InstrumentationScope {
    InstrumentationScope::builder("aether.genai")
        .with_version(version.into())
        .with_schema_url(GENAI_SEMCONV_SCHEMA_URL)
        .build()
}
