use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

use crate::VoiceError;

const MODEL_NAME: &str = "ggml-base.en.bin";

/// Whisper-based speech-to-text transcriber
pub struct Transcriber {
    ctx: Arc<Mutex<WhisperContext>>,
}

impl Transcriber {
    /// Create a new transcriber, loading the Whisper model
    ///
    /// The model is loaded from ~/.config/aether/models/whisper/
    /// If the model doesn't exist, it will be downloaded automatically
    pub fn new() -> Result<Self, VoiceError> {
        let model_path = get_model_path()?;
        Self::with_model_path(model_path)
    }

    /// Create a transcriber with a custom model path
    pub fn with_model_path<P: AsRef<Path>>(model_path: P) -> Result<Self, VoiceError> {
        let path = model_path.as_ref();

        if !path.exists() {
            return Err(VoiceError::ModelNotFound);
        }

        let path_str = path
            .to_str()
            .ok_or_else(|| VoiceError::ModelLoadError("Invalid model path".to_string()))?;

        debug!("Loading Whisper model from {:?}", path);

        let ctx = WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
            .map_err(|e| VoiceError::ModelLoadError(e.to_string()))?;

        info!("Whisper model loaded successfully");

        Ok(Self {
            ctx: Arc::new(Mutex::new(ctx)),
        })
    }

    /// Transcribe audio samples to text
    ///
    /// # Arguments
    /// * `audio` - Audio samples at 16kHz sample rate, mono channel
    ///
    /// # Returns
    /// The transcribed text
    pub async fn transcribe(&self, audio: &[f32]) -> Result<String, VoiceError> {
        if audio.is_empty() {
            return Ok(String::new());
        }

        let sample_count = audio.len();
        debug!("Transcribing {} audio samples", sample_count);

        let ctx = Arc::clone(&self.ctx);
        let audio_vec = audio.to_vec();

        // Use spawn_blocking to avoid blocking the async runtime
        let result = tokio::task::spawn_blocking(move || {
            let ctx = ctx.blocking_lock();

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

            // Set language to English
            params.set_language(Some("en"));
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);

            let mut state = ctx
                .create_state()
                .map_err(|e| VoiceError::TranscriptionError(e.to_string()))?;

            state
                .full(params, &audio_vec)
                .map_err(|e| VoiceError::TranscriptionError(e.to_string()))?;

            let num_segments = state
                .full_n_segments()
                .map_err(|e| VoiceError::TranscriptionError(e.to_string()))?;

            let mut result = String::new();

            for i in 0..num_segments {
                let segment = state
                    .full_get_segment_text(i)
                    .map_err(|e| VoiceError::TranscriptionError(e.to_string()))?;

                if i > 0 {
                    result.push(' ');
                }
                result.push_str(&segment);
            }

            Ok(result.trim().to_string())
        })
        .await
        .map_err(|e| VoiceError::TranscriptionError(format!("Task join error: {}", e)))??;

        debug!("Transcription result: {}", result);

        Ok(result)
    }
}

/// Get the directory where models should be stored
pub fn get_model_dir() -> Result<PathBuf, VoiceError> {
    dirs::config_dir()
        .map(|dir| dir.join("aether").join("models").join("whisper"))
        .ok_or_else(|| VoiceError::ModelLoadError("Could not find config directory".to_string()))
}

/// Get the path to the Whisper model file
fn get_model_path() -> Result<PathBuf, VoiceError> {
    get_model_dir().map(|dir| dir.join(MODEL_NAME))
}

/// Download the Whisper model from Hugging Face
pub async fn download_model() -> Result<PathBuf, VoiceError> {
    let model_dir = get_model_dir()?;

    // Create directory if it doesn't exist
    tokio::fs::create_dir_all(&model_dir)
        .await
        .map_err(|e| VoiceError::ModelDownloadError(e.to_string()))?;

    let model_path = model_dir.join(MODEL_NAME);

    if model_path.exists() {
        info!("Model already exists at {:?}", model_path);
        return Ok(model_path);
    }

    info!("Downloading Whisper model to {:?}", model_path);

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        MODEL_NAME
    );

    let response = reqwest::get(&url)
        .await
        .map_err(|e| VoiceError::ModelDownloadError(e.to_string()))?;

    if !response.status().is_success() {
        return Err(VoiceError::ModelDownloadError(format!(
            "Failed to download model: HTTP {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| VoiceError::ModelDownloadError(e.to_string()))?;

    tokio::fs::write(&model_path, &bytes)
        .await
        .map_err(|e| VoiceError::ModelDownloadError(e.to_string()))?;

    info!("Model downloaded successfully");

    Ok(model_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_model_path() {
        let path = get_model_path();
        assert!(path.is_ok());
        let path = path.unwrap();
        assert!(path.to_string_lossy().contains("aether"));
        assert!(path.to_string_lossy().contains("whisper"));
        assert!(path.to_string_lossy().ends_with(MODEL_NAME));
    }

    #[test]
    fn test_transcribe_empty_audio() {
        // This test requires the model file to exist, so we'll skip it in CI
        if let Ok(transcriber) = Transcriber::new() {
            let result = transcriber.transcribe(&[]);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), "");
        }
    }
}
