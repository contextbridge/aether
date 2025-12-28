/// Represents the current state of the voice recording system
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordingState {
    /// No recording in progress
    Idle,
    /// Currently recording audio
    Recording,
    /// Processing recorded audio into text
    Transcribing,
    /// An error occurred
    Error(String),
}

impl RecordingState {
    /// Check if recording can be started from this state
    pub fn can_start_recording(&self) -> bool {
        matches!(self, RecordingState::Idle | RecordingState::Error(_))
    }

    /// Check if recording can be stopped from this state
    pub fn can_stop_recording(&self) -> bool {
        matches!(self, RecordingState::Recording)
    }

    /// Check if the system is busy (recording or transcribing)
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            RecordingState::Recording | RecordingState::Transcribing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recording_state_transitions() {
        let idle = RecordingState::Idle;
        assert!(idle.can_start_recording());
        assert!(!idle.can_stop_recording());
        assert!(!idle.is_busy());

        let recording = RecordingState::Recording;
        assert!(!recording.can_start_recording());
        assert!(recording.can_stop_recording());
        assert!(recording.is_busy());

        let transcribing = RecordingState::Transcribing;
        assert!(!transcribing.can_start_recording());
        assert!(!transcribing.can_stop_recording());
        assert!(transcribing.is_busy());

        let error = RecordingState::Error("test error".to_string());
        assert!(error.can_start_recording());
        assert!(!error.can_stop_recording());
        assert!(!error.is_busy());
    }
}
