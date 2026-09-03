use aether_telemetry::ContentCaptureSettings;

pub const SYSTEM_PROMPT: &str = "You are the test agent.";
pub const SYSTEM_PROMPT_SHA256: &str = "5304558d6e33e0cf4e4e71c7a6dfde26fac43e41560acbd6289cbcfa04af7719";
pub const SYSTEM_INSTRUCTIONS_JSON: &str = r#"[{"type":"text","content":"You are the test agent."}]"#;

pub fn all_content() -> ContentCaptureSettings {
    ContentCaptureSettings {
        system_instructions: true,
        input_messages: true,
        output_messages: true,
        tool_definitions: true,
        tool_calls: true,
    }
}
