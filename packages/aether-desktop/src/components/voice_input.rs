use dioxus::prelude::*;
use tokio::sync::oneshot;
use tracing::{debug, error, warn};

use aether_voice::{download_model, RecordingState};

#[component]
pub fn VoiceInput(on_transcription: EventHandler<String>, disabled: bool) -> Element {
    let mut recording_state = use_signal(|| RecordingState::Idle);
    let mut stop_tx = use_signal(|| None::<oneshot::Sender<()>>);

    let toggle_recording = move |_| {
        let state = recording_state.read().clone();

        if state.can_start_recording() {
            // Start recording
            debug!("Starting voice recording");
            recording_state.set(RecordingState::Recording);

            let (tx, rx) = oneshot::channel();
            stop_tx.set(Some(tx));

            spawn(async move {
                match aether_voice::record_and_transcribe(rx).await {
                    Ok(text) => {
                        if !text.is_empty() {
                            debug!("Transcription complete: {}", text);
                            on_transcription.call(text);
                        }
                        recording_state.set(RecordingState::Idle);
                    }
                    Err(e) => {
                        error!("Voice recording error: {}", e);
                        let error_msg = if e.to_string().contains("ModelNotFound") {
                            "Whisper model not found. Click to download.".to_string()
                        } else {
                            format!("Error: {}", e)
                        };
                        recording_state.set(RecordingState::Error(error_msg));
                    }
                }
            });
        } else if state.can_stop_recording() {
            // Stop recording
            debug!("Stopping voice recording");
            if let Some(tx) = stop_tx.write().take() {
                let _ = tx.send(());
            }
            recording_state.set(RecordingState::Transcribing);
        }
    };

    let download_model_handler = move |_| {
        debug!("Starting model download");
        recording_state.set(RecordingState::Transcribing);

        spawn(async move {
            match download_model().await {
                Ok(_) => {
                    debug!("Model downloaded successfully");
                    recording_state.set(RecordingState::Idle);
                }
                Err(e) => {
                    error!("Model download error: {}", e);
                    recording_state.set(RecordingState::Error(format!(
                        "Download failed: {}",
                        e
                    )));
                }
            }
        });
    };

    let state = recording_state.read().clone();
    let is_disabled = disabled || state.is_busy();

    let (button_class, button_title, icon_element) = match &state {
        RecordingState::Idle => (
            "text-gray-400 hover:text-white hover:bg-white/10",
            "Start voice recording",
            rsx! {
                // Microphone icon
                path {
                    d: "M12 1a3 3 0 0 0-3 3v8a3 3 0 0 0 6 0V4a3 3 0 0 0-3-3z"
                }
                path {
                    d: "M19 10v2a7 7 0 0 1-14 0v-2"
                }
                line {
                    x1: "12",
                    y1: "19",
                    x2: "12",
                    y2: "23"
                }
                line {
                    x1: "8",
                    y1: "23",
                    x2: "16",
                    y2: "23"
                }
            },
        ),
        RecordingState::Recording => (
            "text-red-500 bg-red-500/20 recording-active",
            "Stop recording",
            rsx! {
                // Stop icon (square)
                rect {
                    x: "6",
                    y: "6",
                    width: "12",
                    height: "12",
                    rx: "2"
                }
            },
        ),
        RecordingState::Transcribing => (
            "text-blue-400 opacity-50 cursor-wait",
            "Processing...",
            rsx! {
                // Processing spinner
                circle {
                    class: "opacity-25",
                    cx: "12",
                    cy: "12",
                    r: "10",
                    stroke: "currentColor",
                    stroke_width: "4"
                }
                path {
                    class: "opacity-75",
                    fill: "currentColor",
                    d: "M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4zm2 5.291A7.962 7.962 0 014 12H0c0 3.042 1.135 5.824 3 7.938l3-2.647z"
                }
            },
        ),
        RecordingState::Error(msg) => {
            if msg.contains("download") {
                (
                    "text-yellow-400 hover:text-yellow-300 hover:bg-yellow-400/10",
                    msg.as_str(),
                    rsx! {
                        // Download icon
                        path {
                            d: "M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"
                        }
                        polyline {
                            points: "7 10 12 15 17 10"
                        }
                        line {
                            x1: "12",
                            y1: "15",
                            x2: "12",
                            y2: "3"
                        }
                    },
                )
            } else {
                (
                    "text-red-400 hover:text-red-300 hover:bg-red-400/10",
                    msg.as_str(),
                    rsx! {
                        // Error icon
                        circle {
                            cx: "12",
                            cy: "12",
                            r: "10"
                        }
                        line {
                            x1: "12",
                            y1: "8",
                            x2: "12",
                            y2: "12"
                        }
                        line {
                            x1: "12",
                            y1: "16",
                            x2: "12.01",
                            y2: "16"
                        }
                    },
                )
            }
        }
    };

    rsx! {
        button {
            class: "transition-all p-2 rounded-lg {button_class}",
            onclick: if matches!(state, RecordingState::Error(ref msg) if msg.contains("download")) {
                download_model_handler
            } else {
                toggle_recording
            },
            disabled: is_disabled,
            title: button_title,
            svg {
                xmlns: "http://www.w3.org/2000/svg",
                width: "20",
                height: "20",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                {icon_element}
            }
        }
    }
}
