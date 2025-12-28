# aether-voice

Voice input support for Aether using local Whisper transcription.

## Features

- Cross-platform audio recording using cpal
- Local speech-to-text using Whisper.cpp
- Privacy-focused: all processing happens locally
- No API keys or cloud services required

## System Requirements

### Linux

Install ALSA development libraries:

```bash
# Ubuntu/Debian
sudo apt-get install libasound2-dev

# Fedora
sudo dnf install alsa-lib-devel

# Arch
sudo pacman -S alsa-lib
```

### macOS

No additional requirements (uses CoreAudio).

### Windows

No additional requirements (uses WASAPI).

## Model Files

The Whisper model (`ggml-base.en.bin`, ~141MB) will be automatically downloaded on first use to:

- **Linux/macOS**: `~/.config/aether/models/whisper/`
- **Windows**: `%APPDATA%\aether\models\whisper\`

You can manually download models from:
https://huggingface.co/ggerganov/whisper.cpp/tree/main

## Usage

```rust
use aether_voice::{AudioRecorder, Transcriber, record_and_transcribe};
use tokio::sync::oneshot;

// Simple usage
let (tx, rx) = oneshot::channel();
// Start recording, then call tx.send(()) to stop
let text = record_and_transcribe(rx).await?;

// Advanced usage
let recorder = AudioRecorder::new()?;
let audio = recorder.record(Duration::from_secs(5)).await?;

let transcriber = Transcriber::new()?;
let text = transcriber.transcribe(&audio)?;
```

## Performance

- Model loading: ~1-3 seconds (first use only)
- Transcription: ~0.5-2s for typical prompt (10-30 seconds of speech)
- Memory: ~500MB during transcription
- Audio is resampled to 16kHz mono (Whisper's required format)

## Privacy

All audio processing happens locally on your machine. Audio data never leaves your device.
