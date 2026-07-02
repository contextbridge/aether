// The upstream GenAI semantic conventions are still in development and every
// constant is marked deprecated; this module centralizes the allow so call
// sites stay clean.
#![allow(deprecated)]

pub use opentelemetry_semantic_conventions::SCHEMA_URL as GENAI_SEMCONV_SCHEMA_URL;

pub const GENAI_RESPONSE_START_EVENT: &str = "gen_ai.response.start";
pub const GENAI_TOOL_CALL_START_EVENT: &str = "gen_ai.tool_call.start";
pub const EXCEPTION_EVENT: &str = "exception";
pub const MESSAGE_ID: &str = "message.id";
pub const TOOL_CALL_ID: &str = "tool_call.id";
pub const TOOL_CALL_NAME: &str = "tool_call.name";
pub const LLM_ATTEMPT: &str = "aether.llm.attempt";
pub const LLM_PURPOSE: &str = "aether.llm.purpose";

pub fn provider_registry_name(provider_id: &str) -> &str {
    match provider_id {
        "anthropic" => "anthropic",
        "bedrock" => "aws.bedrock",
        "gemini" => "gcp.gen_ai",
        "openai" | "codex" => "openai",
        "openrouter" => "openrouter",
        "ollama" => "ollama",
        "llamacpp" => "llama.cpp",
        "deepseek" => "deepseek",
        "moonshot" => "moonshot",
        "zai" => "zai",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_provider_ids_to_registry_names() {
        assert_eq!(provider_registry_name("anthropic"), "anthropic");
        assert_eq!(provider_registry_name("bedrock"), "aws.bedrock");
        assert_eq!(provider_registry_name("llamacpp"), "llama.cpp");
        assert_eq!(provider_registry_name("custom"), "custom");
    }
}
