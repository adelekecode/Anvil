//! Sample-rate conversion and channel downmix to Anvil's canonical format.
//!
//! The capture device may deliver 44.1 kHz stereo, 16 kHz mono, or anything
//! the OS and hardware negotiate. The Opus encoder only accepts 48 kHz mono
//! frames of exactly 960 samples. This module is the adapter between them.
//!
//! ```text
//!   device PCM (rate R, channels C)   →   48 kHz mono
//! ```
//!
//! ## Resampling
//!
//! Linear interpolation. It is not audiophile-grade, but for a voice codec
//! running at 24 kbps it is transparent, and it costs a fraction of the CPU
//! of a sinc resampler. If listening tests ever say otherwise, the interface
//! below is the place to swap in a higher-quality implementation — the
//! caller never touches the resample internals.
//!
//! ## Downmix
//!
//! Stereo → mono is a simple average: `(L + R) / 2`. This preserves the
//! combined signal energy without clipping; the subsequent Opus encode does
//! not clip `i16` input anyway, but keeping the signal in range avoids a
//! loudness jump between stereo and mono devices.

use std::collections::VecDeque;

/// The canonical internal sample rate for Opus encoding.
pub const TARGET_SAMPLE_RATE: u32 = 48_000;

/// The canonical internal channel count.
pub const TARGET_CHANNELS: u8 = 1;

/// Samples per Opus frame at the target rate (20 ms × 48 kHz).
pub const TARGET_FRAME_SAMPLES: usize = 960;

/// Converts capture device PCM to 48 kHz mono.
///
/// Accepts a stream of interleaved samples at any rate / channel count and
/// emits batches that, when accumulated, form complete 960-sample mono
/// frames ready for the Opus encoder.
pub struct AudioResampler {
    /// Capture device sample rate.
    input_rate: u32,
    /// Capture device channel count.
    input_channels: u8,
    /// Fractional input-sample position relative to the first sample in
    /// `input`. Keeping this position relative to the deque avoids mixing
    /// interleaved and mono indices at chunk boundaries.
    phase: f64,
    /// Downmixed mono input waiting for the next interpolation point.
    input: VecDeque<i16>,
    /// Incomplete interleaved input frame from the previous push.
    partial: Vec<i16>,
    /// Accumulated output samples that did not yet form a complete Opus
    /// frame. The caller drains this via [`Self::drain_frame`].
    pending: VecDeque<i16>,
}

impl AudioResampler {
    /// Build a resampler for the given device configuration.
    #[must_use]
    pub fn new(input_rate: u32, input_channels: u8) -> Self {
        Self {
            input_rate,
            input_channels,
            phase: 0.0,
            input: VecDeque::with_capacity(4_096),
            partial: Vec::with_capacity(input_channels as usize),
            pending: VecDeque::with_capacity(4_096),
        }
    }

    /// Feed a chunk of interleaved device PCM into the resampler.
    ///
    /// Output is appended to the internal `pending` queue. The caller should
    /// call [`Self::drain_frame`] afterwards to pull complete 960-sample
    /// frames.
    pub fn push(&mut self, samples: &[i16]) {
        let in_channels = self.input_channels as usize;
        if in_channels == 0 || samples.is_empty() {
            return;
        }
        let mut data = Vec::with_capacity(self.partial.len() + samples.len());
        data.extend_from_slice(&self.partial);
        data.extend_from_slice(samples);
        let complete_len = data.len() / in_channels * in_channels;
        self.partial.clear();
        self.partial.extend_from_slice(&data[complete_len..]);

        for frame in data[..complete_len].chunks_exact(in_channels) {
            let sum: i64 = frame.iter().map(|sample| i64::from(*sample)).sum();
            self.input.push_back((sum / in_channels as i64) as i16);
        }

        // At the native rate there is no interpolation to perform. Emitting
        // immediately also avoids waiting for a look-ahead sample at the end
        // of every otherwise-perfect 20 ms capture block.
        if self.input_rate == TARGET_SAMPLE_RATE {
            self.pending.extend(self.input.drain(..));
            self.phase = 0.0;
            return;
        }

        let step = self.input_rate as f64 / TARGET_SAMPLE_RATE as f64;
        if !step.is_finite() || step <= 0.0 {
            self.input.clear();
            self.phase = 0.0;
            return;
        }

        // Keep one source sample as interpolation look-ahead. This introduces
        // at most one input sample of latency and avoids interpolating the
        // first real sample against synthetic zeroes.
        while self.phase + 1.0 < self.input.len() as f64 {
            let index = self.phase.floor() as usize;
            let fraction = self.phase - index as f64;
            let a = f64::from(self.input[index]);
            let b = f64::from(self.input[index + 1]);
            let sample = a + (b - a) * fraction;
            self.pending
                .push_back(sample.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16);

            self.phase += step;
            let consumed = self.phase.floor() as usize;
            if consumed > 0 {
                self.input.drain(..consumed.min(self.input.len()));
                self.phase -= consumed as f64;
            }
        }
    }

    /// Pull one complete 960-sample mono frame if enough samples have
    /// accumulated. Returns `None` otherwise.
    pub fn drain_frame(&mut self) -> Option<Vec<i16>> {
        if self.pending.len() < TARGET_FRAME_SAMPLES {
            return None;
        }
        let frame: Vec<i16> = self.pending.drain(..TARGET_FRAME_SAMPLES).collect();
        Some(frame)
    }

    /// How many complete frames are buffered and ready to drain.
    #[must_use]
    pub fn ready_frames(&self) -> usize {
        self.pending.len() / TARGET_FRAME_SAMPLES
    }

    /// Reset internal state (e.g. after a device reconfiguration).
    pub fn reset(&mut self) {
        self.phase = 0.0;
        self.input.clear();
        self.partial.clear();
        self.pending.clear();
    }

    /// Nominal target frame size at the output rate.
    #[must_use]
    pub const fn target_frame_samples(&self) -> usize {
        TARGET_FRAME_SAMPLES
    }
}

impl core::fmt::Debug for AudioResampler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AudioResampler")
            .field("input_rate", &self.input_rate)
            .field("input_channels", &self.input_channels)
            .field("pending", &self.pending.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_at_48khz_mono_produces_same_samples() {
        // When input already matches target, the resampler should be nearly
        // transparent. Each 960-sample chunk should pass through.
        let mut rs = AudioResampler::new(48_000, 1);
        let input: Vec<i16> = (0..1920).map(|i| (i % 200) as i16).collect();
        rs.push(&input[..500]);
        rs.push(&input[500..1000]);
        rs.push(&input[1000..1920]);

        // Should have at least 1 full frame.
        let frame = rs.drain_frame().unwrap();
        assert_eq!(frame.len(), 960);
        // First few samples should match input closely (linear interp at
        // same rate is identity).
        for i in 0..10 {
            assert_eq!(frame[i], input[i]);
        }
    }

    #[test]
    fn resampling_44khz_stereo_to_48khz_mono() {
        // 44.1 kHz stereo → 48 kHz mono. Each 20 ms mono frame = 960
        // samples. At 44.1 kHz stereo: 20 ms = 882 samples × 2 channels =
        // 1,764 interleaved. The resampler should produce ~960 mono samples
        // from roughly that many input samples.
        let mut rs = AudioResampler::new(44_100, 2);
        // Generate a second of stereo silence: we're only testing sample
        // count, not audio quality.
        let stereo: Vec<i16> = vec![0i16; 44_100 * 2]; // 1 second of stereo
        rs.push(&stereo);

        let frames = rs.ready_frames();
        assert!(frames >= 30, "expected 30+ frames (600+ ms) from 1s of input, got {frames}");

        for _ in 0..frames {
            let frame = rs.drain_frame().unwrap();
            assert_eq!(frame.len(), 960);
        }
    }

    #[test]
    fn resampling_16khz_mono_to_48khz_mono() {
        // 16 kHz mono: 20 ms = 320 samples. The resampler should upscale.
        let mut rs = AudioResampler::new(16_000, 1);
        let input: Vec<i16> = (0..16000).map(|i| (i % 100) as i16).collect();
        rs.push(&input);

        assert!(rs.ready_frames() > 0, "should have produced at least one frame");
        let frame = rs.drain_frame().unwrap();
        assert_eq!(frame.len(), 960);
    }

    #[test]
    fn drain_returns_none_when_not_enough_samples() {
        let mut rs = AudioResampler::new(48_000, 1);
        rs.push(&vec![0i16; 500]);
        assert!(rs.drain_frame().is_none());
        // Push the rest.
        rs.push(&vec![0i16; 500]);
        assert!(rs.drain_frame().is_some());
    }

    #[test]
    fn reset_clears_pending_and_fraction() {
        let mut rs = AudioResampler::new(44_100, 2);
        rs.push(&vec![100i16; 10_000]);
        assert!(rs.ready_frames() > 0);
        rs.reset();
        assert_eq!(rs.ready_frames(), 0);
        assert_eq!(rs.phase, 0.0);
    }

    #[test]
    fn downmix_stereo_to_mono_averages_channels() {
        // Two identical sine waves → mono should match one channel exactly.
        let mut rs = AudioResampler::new(48_000, 2);
        let stereo: Vec<i16> = (0..1920)
            .flat_map(|i| {
                let s = i as i16 % 100;
                [s, s] // L = R
            })
            .collect();
        rs.push(&stereo);
        let frame = rs.drain_frame().unwrap();
        // First sample should match the input's first left channel.
        assert_eq!(frame[0], 0);
        assert_eq!(frame[1], 1);
    }

    #[test]
    fn empty_input_produces_no_output() {
        let mut rs = AudioResampler::new(48_000, 1);
        rs.push(&[]);
        assert!(rs.drain_frame().is_none());
    }

    #[test]
    fn frame_count_consistent_for_a_known_duration() {
        // 1 second of 48 kHz stereo = 96,000 interleaved samples.
        // At 48 kHz mono 20 ms: 50 frames expected.
        let mut rs = AudioResampler::new(48_000, 2);
        let stereo: Vec<i16> = vec![0i16; 96_000];
        rs.push(&stereo);
        let mut count = 0;
        while let Some(frame) = rs.drain_frame() {
            assert_eq!(frame.len(), 960);
            count += 1;
        }
        assert!(count >= 45, "expected ~50 frames from 1s stereo, got {count}");
        assert!(count <= 55, "expected ~50 frames, got {count}");
    }
}
