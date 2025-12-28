use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, Host, SampleFormat, StreamConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing::{debug, warn};

use crate::VoiceError;

const TARGET_SAMPLE_RATE: u32 = 16000; // Whisper requires 16kHz

/// Audio recorder using cpal for cross-platform audio capture
pub struct AudioRecorder {
    host: Host,
    device: Device,
    config: StreamConfig,
}

impl AudioRecorder {
    /// Create a new audio recorder with the default input device
    pub fn new() -> Result<Self, VoiceError> {
        let host = cpal::default_host();

        let device = host
            .default_input_device()
            .ok_or(VoiceError::NoInputDevice)?;

        debug!("Using input device: {:?}", device.name());

        let config = device
            .default_input_config()
            .map_err(|e| VoiceError::StreamError(e.to_string()))?;

        debug!("Default input config: {:?}", config);

        Ok(Self {
            host,
            device,
            config: config.into(),
        })
    }

    /// Record audio for a specific duration
    pub async fn record(&self, duration: Duration) -> Result<Vec<f32>, VoiceError> {
        let (tx, rx) = oneshot::channel();

        // Schedule stop signal
        tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            let _ = tx.send(());
        });

        self.record_until_stopped(rx).await
    }

    /// Record audio until the stop signal is received
    pub async fn record_until_stopped(
        &self,
        mut stop_rx: oneshot::Receiver<()>,
    ) -> Result<Vec<f32>, VoiceError> {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let samples_clone = Arc::clone(&samples);

        let err_fn = |err| {
            warn!("Audio stream error: {}", err);
        };

        let sample_format = self.config.sample_format;
        let channels = self.config.channels;
        let sample_rate = self.config.sample_rate.0;

        let stream = match sample_format {
            SampleFormat::F32 => self.device.build_input_stream(
                &self.config,
                move |data: &[f32], _: &_| {
                    samples_clone.lock().unwrap().extend_from_slice(data);
                },
                err_fn,
                None,
            ),
            SampleFormat::I16 => {
                let samples_clone = Arc::clone(&samples);
                self.device.build_input_stream(
                    &self.config,
                    move |data: &[i16], _: &_| {
                        let mut samples = samples_clone.lock().unwrap();
                        for &sample in data {
                            samples.push(sample as f32 / i16::MAX as f32);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            SampleFormat::U16 => {
                let samples_clone = Arc::clone(&samples);
                self.device.build_input_stream(
                    &self.config,
                    move |data: &[u16], _: &_| {
                        let mut samples = samples_clone.lock().unwrap();
                        for &sample in data {
                            let normalized = (sample as f32 / u16::MAX as f32) * 2.0 - 1.0;
                            samples.push(normalized);
                        }
                    },
                    err_fn,
                    None,
                )
            }
            _ => {
                return Err(VoiceError::InvalidAudioFormat(format!(
                    "Unsupported sample format: {:?}",
                    sample_format
                )))
            }
        }
        .map_err(|e| VoiceError::StreamError(e.to_string()))?;

        stream
            .play()
            .map_err(|e| VoiceError::StreamError(e.to_string()))?;

        debug!("Recording started");

        // Wait for stop signal
        let _ = stop_rx.await;

        debug!("Recording stopped");

        drop(stream);

        let mut audio_data = samples.lock().unwrap().clone();

        // Convert to mono if stereo
        if channels > 1 {
            audio_data = convert_to_mono(&audio_data, channels as usize);
        }

        // Resample to 16kHz if needed
        if sample_rate != TARGET_SAMPLE_RATE {
            audio_data = resample(&audio_data, sample_rate, TARGET_SAMPLE_RATE);
        }

        debug!("Captured {} samples at 16kHz", audio_data.len());

        Ok(audio_data)
    }
}

/// Convert multi-channel audio to mono by averaging channels
fn convert_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    samples
        .chunks(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Simple linear resampling
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;

    (0..output_len)
        .map(|i| {
            let src_idx = (i as f64 * ratio) as usize;
            if src_idx < samples.len() {
                samples[src_idx]
            } else {
                0.0
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_to_mono() {
        let stereo = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let mono = convert_to_mono(&stereo, 2);
        assert_eq!(mono.len(), 3);
        assert!((mono[0] - 0.15).abs() < 0.001);
        assert!((mono[1] - 0.35).abs() < 0.001);
        assert!((mono[2] - 0.55).abs() < 0.001);
    }

    #[test]
    fn test_resample_same_rate() {
        let samples = vec![0.1, 0.2, 0.3, 0.4];
        let resampled = resample(&samples, 16000, 16000);
        assert_eq!(samples, resampled);
    }

    #[test]
    fn test_resample_downsample() {
        let samples = vec![0.1, 0.2, 0.3, 0.4];
        let resampled = resample(&samples, 32000, 16000);
        assert_eq!(resampled.len(), 2);
    }
}
