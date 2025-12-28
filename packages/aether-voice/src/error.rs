use thiserror::Error;

#[derive(Error, Debug)]
pub enum VoiceError {
    #[error("No audio input device found")]
    NoInputDevice,

    #[error("Audio stream error: {0}")]
    StreamError(String),

    #[error("Transcription error: {0}")]
    TranscriptionError(String),

    #[error("Whisper model not found")]
    ModelNotFound,

    #[error("Failed to load Whisper model: {0}")]
    ModelLoadError(String),

    #[error("Failed to download model: {0}")]
    ModelDownloadError(String),

    #[error("Invalid audio format: {0}")]
    InvalidAudioFormat(String),

    #[error("Recording cancelled")]
    RecordingCancelled,

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}
