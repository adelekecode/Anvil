//! Capture and playback pipeline workers.
//!
//! These are the non-real-time threads that sit between the CPAL audio
//! callbacks and the Opus codec. The callbacks touch only the ring buffer;
//! every expensive operation — resampling, encoding, decoding — happens
//! here on ordinary threads.

use std::sync::Arc;

use crate::audio::opus::{OpusVoiceDecoder, OpusVoiceEncoder};
use crate::audio::resampler::AudioResampler;
use crate::audio::ring_buffer::PcmRingBuffer;
use crate::Result;

/// The capture pipeline: pull raw device samples from the ring buffer,
/// resample to 48 kHz mono, and encode with Opus.
///
/// One frame of Opus output = one iteration through the loop. The caller
/// decides whether to block-wait for a full frame or yield and come back.
pub struct CapturePipeline {
    encoder: OpusVoiceEncoder,
    resampler: AudioResampler,
}

impl CapturePipeline {
    /// Build from an already-configured encoder and the device's capture
    /// format.
    #[must_use]
    pub fn new(encoder: OpusVoiceEncoder, input_rate: u32, input_channels: u8) -> Self {
        Self { encoder, resampler: AudioResampler::new(input_rate, input_channels) }
    }

    /// Feed raw device samples from the ring buffer into the pipeline.
    /// Returns an encoded Opus payload if a complete frame was produced,
    /// or `None` if more input is needed.
    pub fn push_and_encode(
        &mut self,
        ring: &PcmRingBuffer,
        frame_buffer: &mut Vec<i16>,
    ) -> Result<Option<Vec<u8>>> {
        let frame_size = self.encoder.frame_samples_per_channel();
        frame_buffer.resize(frame_size * 2, 0); // Worst-case: stereo before downmix

        let read = ring.read(frame_buffer);
        if read > 0 {
            self.resampler.push(&frame_buffer[..read]);
        }

        if let Some(mono) = self.resampler.drain_frame() {
            let encoded = self.encoder.encode_frame(&mono)?;
            if encoded.is_empty() {
                return Ok(None);
            }
            return Ok(Some(encoded.payload));
        }
        Ok(None)
    }

    /// Drain all complete frames buffered in the resampler. Returns zero or
    /// more encoded Opus payloads.
    pub fn drain_all(
        &mut self,
        ring: &PcmRingBuffer,
        frame_buffer: &mut Vec<i16>,
    ) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        let frame_size = self.encoder.frame_samples_per_channel();
        frame_buffer.resize(frame_size * 2, 0);

        let read = ring.read(frame_buffer);
        if read > 0 {
            self.resampler.push(&frame_buffer[..read]);
        }

        while let Some(mono) = self.resampler.drain_frame() {
            let encoded = self.encoder.encode_frame(&mono)?;
            if !encoded.is_empty() {
                out.push(encoded.payload);
            }
        }
        Ok(out)
    }

    /// The encoder's configuration.
    #[must_use]
    pub const fn encoder(&self) -> &OpusVoiceEncoder {
        &self.encoder
    }

    /// Mutable access to the encoder for runtime bitrate / FEC changes.
    #[must_use]
    pub fn encoder_mut(&mut self) -> &mut OpusVoiceEncoder {
        &mut self.encoder
    }

    /// Reset after a device change (new sample rate / channel count).
    pub fn reconfigure(&mut self, input_rate: u32, input_channels: u8) {
        self.resampler = AudioResampler::new(input_rate, input_channels);
    }
}

impl core::fmt::Debug for CapturePipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CapturePipeline")
            .field("encoder", &self.encoder)
            .field("resampler", &self.resampler)
            .finish_non_exhaustive()
    }
}

/// The playback pipeline: receive Opus packets, decode to PCM, and write
/// to the ring buffer for the CPAL output callback to consume.
pub struct PlaybackPipeline {
    decoder: OpusVoiceDecoder,
    /// Scratch buffer reused across decode calls.
    decode_scratch: Vec<i16>,
}

impl PlaybackPipeline {
    /// Build from an already-configured decoder.
    #[must_use]
    pub fn new(decoder: OpusVoiceDecoder) -> Self {
        Self {
            decoder,
            decode_scratch: Vec::new(),
        }
    }

    /// Decode one Opus packet and push its PCM to the ring buffer.
    /// Returns the number of samples pushed.
    pub fn decode_and_push(
        &mut self,
        packet: &[u8],
        ring: &PcmRingBuffer,
    ) -> Result<usize> {
        let decoded = self.decoder.decode_frame(Some(packet), false)?;
        let len = decoded.samples.len();
        ring.write(&decoded.samples);
        Ok(len)
    }

    /// Push a concealment frame (PLC or FEC) to the ring buffer.
    /// Returns the number of samples pushed.
    pub fn conceal_and_push(
        &mut self,
        fec_packet: Option<&[u8]>,
        ring: &PcmRingBuffer,
    ) -> Result<usize> {
        let fec = fec_packet.is_some();
        let decoded = self.decoder.decode_frame(fec_packet, fec)?;
        let len = decoded.samples.len();
        ring.write(&decoded.samples);
        Ok(len)
    }

    /// The decoder's configuration.
    #[must_use]
    pub const fn decoder(&self) -> &OpusVoiceDecoder {
        &self.decoder
    }
}

impl core::fmt::Debug for PlaybackPipeline {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PlaybackPipeline")
            .field("decoder", &self.decoder)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::opus::OpusConfig;

    fn voice_config() -> OpusConfig {
        OpusConfig {
            sample_rate: 48_000,
            channels: 1,
            frame_duration_ms: 20,
            bitrate: 24_000,
            fec: true,
            dtx: false,
            expected_packet_loss: 0,
            complexity: 0,
        }
    }

    fn sine(steps: usize) -> Vec<i16> {
        (0..steps)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                ((2.0 * core::f64::consts::PI * 440.0 * t).sin() * 16_000.0) as i16
            })
            .collect()
    }

    #[test]
    fn capture_pipeline_passthrough_produces_encoded_frames() {
        let enc = OpusVoiceEncoder::new(voice_config()).unwrap();
        let mut pipeline = CapturePipeline::new(enc, 48_000, 1);

        let ring = Arc::new(PcmRingBuffer::new(96_000));
        let mut buf = Vec::new();

        // Write enough 48 kHz mono PCM to produce several frames.
        let pcm = sine(4800);
        ring.write(&pcm);

        let mut frames = 0;
        for _ in 0..10 {
            if let Ok(Some(payload)) = pipeline.push_and_encode(&ring, &mut buf) {
                assert!(!payload.is_empty());
                frames += 1;
            } else {
                break; // No more output — ring drained.
            }
        }
        assert!(frames >= 3, "expected 3+ frames from 4800 samples, got {frames}");
    }

    #[test]
    fn capture_resamples_44khz_stereo() {
        let enc = OpusVoiceEncoder::new(voice_config()).unwrap();
        let mut pipeline = CapturePipeline::new(enc, 44_100, 2);

        let ring = Arc::new(PcmRingBuffer::new(200_000));
        let mut buf = Vec::new();

        // 1 second of 44.1 kHz stereo silence: 88,200 interleaved samples.
        ring.write(&vec![0i16; 88_200]);

        let frames = pipeline.drain_all(&ring, &mut buf).unwrap();
        assert!(frames.len() >= 40, "expected 40+ frames from 1s of 44.1k, got {}", frames.len());
    }

    #[test]
    fn playback_pipeline_roundtrips_through_codec() {
        let enc = OpusVoiceEncoder::new(voice_config()).unwrap();
        let dec = OpusVoiceDecoder::new(voice_config()).unwrap();

        let mut cap = CapturePipeline::new(enc, 48_000, 1);
        let mut play = PlaybackPipeline::new(dec);

        let ring = Arc::new(PcmRingBuffer::new(96_000));
        let mut buf = Vec::new();

        ring.write(&sine(960));
        let encoded = cap.push_and_encode(&ring, &mut buf).unwrap().unwrap();

        let output_ring = Arc::new(PcmRingBuffer::new(96_000));
        play.decode_and_push(&encoded, &output_ring).unwrap();

        let mut out = vec![0i16; 960];
        output_ring.read(&mut out);
        // After encode → decode, samples should not be all zero.
        let nonzero = out.iter().filter(|s| **s != 0).count();
        assert!(nonzero > 100, "roundtripped audio is silent");
    }
}
