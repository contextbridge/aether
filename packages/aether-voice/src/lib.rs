mod error;
mod recorder;
mod state;
mod transcriber;

pub use error::VoiceError;
pub use recorder::AudioRecorder;
pub use state::RecordingState;
pub use transcriber::Transcriber;

use tokio::sync::oneshot;

/// Record audio and transcribe it to text.
///
/// This is a convenience function that combines recording and transcription.
/// Recording continues until the stop signal is received.
///
/// # Arguments
/// * `stop_rx` - Receiver that signals when to stop recording
///
/// # Returns
/// The transcribed text, or an error if recording or transcription fails
pub async fn record_and_transcribe(
    stop_rx: oneshot::Receiver<()>,
) -> Result<String, VoiceError> {
    let recorder = AudioRecorder::new()?;
    let audio = recorder.record_until_stopped(stop_rx).await?;

    let transcriber = Transcriber::new()?;
    let text = transcriber.transcribe(&audio)?;

    Ok(text)
}
