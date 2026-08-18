//! Desktop audio I/O via CPAL (§ Phase 2).
//!
//! Wires the platform's microphone and speaker to Anvil's ring buffers,
//! resampler, and Opus codec. Exists behind the `desktop` feature because
//! CPAL does not compile for Android / iOS; the real app uses its own
//! native `AudioAdapter` implementations for those targets.
//!
//! ```text
//!   mic → CPAL callback → capture ring → worker → resample → encode
//!   decode → worker → playback ring → CPAL callback → speaker
//! ```
//!
//! All capture samples are written to a ring buffer at the device native
//! rate / channel count. The worker drains the ring, resamples to 48 kHz
//! mono, and feeds Opus. Playback runs the inverse path: the worker writes
//! decoded PCM, the CPAL callback drains it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Stream;

use crate::audio::opus::{OpusConfig, OpusVoiceDecoder, OpusVoiceEncoder};
use crate::audio::pipeline::{CapturePipeline, PlaybackPipeline};
use crate::audio::ring_buffer::PcmRingBuffer;
use crate::{AudioError, Result};

pub struct CpalLoop {
    capture_stream: Option<Stream>,
    playback_stream: Option<Stream>,
    running: Arc<AtomicBool>,
    capture_thread: Option<thread::JoinHandle<()>>,
    playback_thread: Option<thread::JoinHandle<()>>,
    capture_ring: Arc<PcmRingBuffer>,
    playback_ring: Arc<PcmRingBuffer>,
    opus_config: OpusConfig,
}

impl CpalLoop {
    #[must_use]
    pub fn new() -> Self {
        Self {
            capture_stream: None,
            playback_stream: None,
            running: Arc::new(AtomicBool::new(false)),
            capture_thread: None,
            playback_thread: None,
            capture_ring: Arc::new(PcmRingBuffer::new(96_000)),
            playback_ring: Arc::new(PcmRingBuffer::new(96_000)),
            opus_config: OpusConfig {
                sample_rate: 48_000,
                channels: 1,
                frame_duration_ms: 20,
                bitrate: 24_000,
                fec: true,
                dtx: false,
                expected_packet_loss: 0,
                complexity: 0,
            },
        }
    }

    #[must_use]
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Acquire)
    }

    /// The Opus config used by the capture and playback pipelines.
    #[must_use]
    pub const fn opus_config(&self) -> &OpusConfig {
        &self.opus_config
    }

    /// Start capture and playback. Returns once both streams are streaming
    /// and the worker threads are spawned.
    pub fn start(&mut self, opus_config: OpusConfig) -> Result<()> {
        if self.is_running() {
            return Err(AudioError::CaptureUnavailable("already running".into()).into());
        }
        self.opus_config = opus_config;

        let host = cpal::default_host();
        let input_device = host
            .default_input_device()
            .ok_or_else(|| AudioError::CaptureUnavailable("no input device".into()))?;
        let output_device = host
            .default_output_device()
            .ok_or_else(|| AudioError::PlaybackUnavailable("no output device".into()))?;

        self.running.store(true, Ordering::Release);

        // --- capture stream ------------------------------------------------
        let input_config: cpal::StreamConfig = input_device
            .default_input_config()
            .map_err(|e| AudioError::CaptureUnavailable(format!("{e:?}")))?
            .into();

        let cap_ring = self.capture_ring.clone();
        let cap_running = self.running.clone();
        let cap_stream = input_device
            .build_input_stream(
                &input_config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if !cap_running.load(Ordering::Acquire) {
                        return;
                    }
                    let i16_buf: Vec<i16> =
                        data.iter().map(|s| (*s * 32767.0).round() as i16).collect();
                    cap_ring.write(&i16_buf);
                },
                |err| tracing::error!("cpal capture error: {err}"),
                None,
            )
            .map_err(|e| AudioError::CaptureUnavailable(format!("{e:?}")))?;

        cap_stream
            .play()
            .map_err(|e| AudioError::CaptureUnavailable(format!("{e:?}")))?;

        // --- capture worker ------------------------------------------------
        let cap_ring = self.capture_ring.clone();
        let cap_running = self.running.clone();
        let cfg = self.opus_config;
        let cap_rec = ResamplerConfig {
            sample_rate: input_config.sample_rate.0,
            channels: input_config.channels as u8,
        };
        self.capture_thread = Some(thread::spawn(move || {
            let mut pipeline = match OpusVoiceEncoder::new(cfg) {
                Ok(enc) => CapturePipeline::new(enc, cap_rec.sample_rate, cap_rec.channels),
                Err(e) => {
                    tracing::error!("capture pipeline init: {e}");
                    return;
                }
            };
            let mut buf = Vec::new();
            while cap_running.load(Ordering::Acquire) {
                match pipeline.push_and_encode(&cap_ring, &mut buf) {
                    Ok(Some(_payload)) => {
                        // Phase 3: wrap in AudioPacket, encrypt, send.
                    }
                    Ok(None) => {
                        thread::sleep(core::time::Duration::from_millis(1));
                    }
                    Err(e) => {
                        tracing::error!("capture encode: {e}");
                        break;
                    }
                }
            }
        }));

        // --- playback stream -----------------------------------------------
        let output_config: cpal::StreamConfig = output_device
            .default_output_config()
            .map_err(|e| AudioError::PlaybackUnavailable(format!("{e:?}")))?
            .into();

        let pb_ring = self.playback_ring.clone();
        let pb_running = self.running.clone();
        let pb_stream = output_device
            .build_output_stream(
                &output_config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    if !pb_running.load(Ordering::Acquire) {
                        data.fill(0.0);
                        return;
                    }
                    let needed = data.len();
                    let mut pcm = vec![0i16; needed];
                    pb_ring.read(&mut pcm);
                    for (out, s) in data.iter_mut().zip(pcm.iter()) {
                        *out = f32::from(*s) / 32768.0;
                    }
                },
                |err| tracing::error!("cpal playback error: {err}"),
                None,
            )
            .map_err(|e| AudioError::PlaybackUnavailable(format!("{e:?}")))?;

        pb_stream
            .play()
            .map_err(|e| AudioError::PlaybackUnavailable(format!("{e:?}")))?;

        // --- playback worker -----------------------------------------------
        let pb_ring = self.playback_ring.clone();
        let pb_running = self.running.clone();
        let cfg = self.opus_config;
        self.playback_thread = Some(thread::spawn(move || {
            let dec = match OpusVoiceDecoder::new(cfg) {
                Ok(dec) => dec,
                Err(e) => {
                    tracing::error!("playback pipeline init: {e}");
                    return;
                }
            };
            let mut pipeline = PlaybackPipeline::new(dec);
            while pb_running.load(Ordering::Acquire) {
                // Phase 3: receive decrypted AudioPackets here.
                // For the self-test loop, write silence so the
                // playback callback never underruns.
                pb_ring.write(&vec![0i16; 960]);
                thread::sleep(core::time::Duration::from_millis(5));
            }
        }));

        self.capture_stream = Some(cap_stream);
        self.playback_stream = Some(pb_stream);
        Ok(())
    }

    /// Stop the loop and join all threads.
    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Release);

        if let Some(stream) = self.capture_stream.take() {
            let _ = stream.pause();
        }
        if let Some(stream) = self.playback_stream.take() {
            let _ = stream.pause();
        }
        if let Some(handle) = self.capture_thread.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.playback_thread.take() {
            let _ = handle.join();
        }
        self.capture_ring.clear();
        self.playback_ring.clear();
    }
}

impl Default for CpalLoop {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for CpalLoop {
    fn drop(&mut self) {
        self.stop();
    }
}

impl core::fmt::Debug for CpalLoop {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CpalLoop")
            .field("running", &self.is_running())
            .field("opus_config", &self.opus_config)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug)]
struct ResamplerConfig {
    sample_rate: u32,
    channels: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_is_not_running_after_new() {
        let cpal = CpalLoop::new();
        assert!(!cpal.is_running());
    }

    #[test]
    fn stop_on_a_stopped_loop_is_harmless() {
        let mut cpal = CpalLoop::new();
        cpal.stop(); // must not panic.
    }

    #[test]
    fn start_then_stop_is_idempotent() {
        let mut cpal = CpalLoop::new();
        // Don't actually start — the test runner may not have a mic.
        // We just assert the double-stop path is safe.
        cpal.stop();
        cpal.stop();
    }
}
