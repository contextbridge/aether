use crate::output::OutputFormat;
use futures::StreamExt;
use llm::parser::ModelProviderParser;
use llm::types::IsoString;
use llm::{ChatMessage, ContentBlock, Context, LlmError, LlmResponse, StreamingModelProvider};
use std::io::{Read, stdin};
use std::process::ExitCode;
use thiserror::Error;

#[derive(clap::Args)]
pub struct GenerateArgs {
    /// Model to call, as `provider:model` (e.g. `anthropic:claude-sonnet-4-5`).
    #[arg(long)]
    pub model: String,

    /// Prompt text to send.
    #[arg(long, conflicts_with = "prompt_file", required_unless_present = "prompt_file")]
    pub prompt: Option<String>,

    /// Prompt to send from a file path, or `-` to read from stdin.
    #[arg(long, value_name = "PATH_OR_DASH", conflicts_with = "prompt", required_unless_present = "prompt")]
    pub prompt_file: Option<String>,

    /// Optional system prompt.
    #[arg(long)]
    pub system: Option<String>,

    /// Output format. `text` prints the raw response; `json`/`pretty` wrap it as
    /// `{ "text": <response>, "model": <model> }`.
    #[arg(long, default_value = "text")]
    pub output: OutputFormat,
}

#[derive(Debug, Error)]
pub enum GenerateCommandError {
    #[error("provide exactly one of --prompt or --prompt-file")]
    PromptSource,

    #[error("failed to read prompt from {path}: {source}")]
    ReadPrompt { path: String, source: std::io::Error },

    #[error("failed to initialize model `{model}`: {source}")]
    Model { model: String, source: LlmError },

    #[error("model stream error: {0}")]
    Stream(LlmError),
}

/// Call a model with a single prompt and print its response. The judge is a special case of this:
/// the caller supplies a grading prompt and parses the structured verdict out of the response.
pub async fn run(args: GenerateArgs) -> Result<ExitCode, GenerateCommandError> {
    let prompt = resolve_prompt(args.prompt.as_deref(), args.prompt_file.as_deref())?;
    let (provider, _) = ModelProviderParser::default()
        .parse(&args.model)
        .await
        .map_err(|source| GenerateCommandError::Model { model: args.model.clone(), source })?;

    let messages = {
        let mut messages = Vec::new();
        if let Some(system) = args.system.as_deref() {
            messages.push(ChatMessage::System { content: system.to_string(), timestamp: IsoString::now() });
        }
        messages.push(ChatMessage::User { content: vec![ContentBlock::text(&prompt)], timestamp: IsoString::now() });
        messages
    };

    let mut stream = provider.stream_response(&Context::new(messages, vec![]));
    let mut text = String::new();
    while let Some(result) = stream.next().await {
        match result {
            Ok(LlmResponse::Text { chunk }) => text.push_str(&chunk),
            Err(error) => return Err(GenerateCommandError::Stream(error)),
            _ => {}
        }
    }
    println!("{}", format_output(&text, &args.model, args.output));
    Ok(ExitCode::SUCCESS)
}

fn format_output(text: &str, model: &str, format: OutputFormat) -> String {
    match format {
        OutputFormat::Text => text.to_string(),
        OutputFormat::Json => serde_json::json!({ "text": text, "model": model }).to_string(),
        OutputFormat::Pretty => serde_json::to_string_pretty(&serde_json::json!({ "text": text, "model": model }))
            .expect("generate response serializes to JSON"),
    }
}

fn resolve_prompt(prompt: Option<&str>, prompt_file: Option<&str>) -> Result<String, GenerateCommandError> {
    match (prompt, prompt_file) {
        (Some(prompt), None) => Ok(prompt.to_string()),
        (None, Some(prompt_file)) => read_prompt_file(prompt_file),
        _ => Err(GenerateCommandError::PromptSource),
    }
}

fn read_prompt_file(source: &str) -> Result<String, GenerateCommandError> {
    if source == "-" {
        let mut prompt = String::new();
        stdin()
            .read_to_string(&mut prompt)
            .map_err(|error| GenerateCommandError::ReadPrompt { path: "-".to_string(), source: error })?;
        return Ok(prompt);
    }
    std::fs::read_to_string(source)
        .map_err(|error| GenerateCommandError::ReadPrompt { path: source.to_string(), source: error })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_output_wraps_json_and_passes_text_through() {
        assert_eq!(format_output("hi", "anthropic:m", OutputFormat::Text), "hi");

        let json: serde_json::Value =
            serde_json::from_str(&format_output("hi", "anthropic:m", OutputFormat::Json)).unwrap();
        assert_eq!(json["text"], "hi");
        assert_eq!(json["model"], "anthropic:m");
    }

    #[tokio::test]
    async fn run_errors_on_unknown_provider() {
        let dir = tempfile::tempdir().unwrap();
        let prompt_path = dir.path().join("prompt.txt");
        std::fs::write(&prompt_path, "the prompt").unwrap();
        let args = GenerateArgs {
            model: "definitely-not-a-provider:nope".to_string(),
            prompt: None,
            prompt_file: Some(prompt_path.to_string_lossy().into_owned()),
            system: None,
            output: OutputFormat::Json,
        };

        let error = run(args).await.unwrap_err();

        assert!(matches!(error, GenerateCommandError::Model { .. }), "got: {error:?}");
    }

    #[test]
    fn resolve_prompt_uses_inline_prompt() {
        assert_eq!(resolve_prompt(Some("say hi"), None).unwrap(), "say hi");
    }

    #[test]
    fn resolve_prompt_reads_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prompt.txt");
        std::fs::write(&path, "graded prompt").unwrap();

        assert_eq!(resolve_prompt(None, Some(&path.to_string_lossy())).unwrap(), "graded prompt");
    }

    #[test]
    fn resolve_prompt_reports_missing_file() {
        let error = resolve_prompt(None, Some("/nonexistent/prompt.txt")).unwrap_err();

        assert!(matches!(error, GenerateCommandError::ReadPrompt { .. }), "got: {error:?}");
    }

    #[test]
    fn resolve_prompt_rejects_missing_prompt_source() {
        let error = resolve_prompt(None, None).unwrap_err();

        assert!(matches!(error, GenerateCommandError::PromptSource), "got: {error:?}");
    }

    #[test]
    fn resolve_prompt_rejects_multiple_prompt_sources() {
        let error = resolve_prompt(Some("inline"), Some("prompt.txt")).unwrap_err();

        assert!(matches!(error, GenerateCommandError::PromptSource), "got: {error:?}");
    }
}
