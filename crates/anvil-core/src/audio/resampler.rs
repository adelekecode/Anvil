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

use crate::audio::opus::VALID_FRAME_MS;

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
    /// Fractional position in the input stream, scaled by `input_rate` so
    /// that incrementing by `TARGET_SAMPLE_RATE` moves one output sample
    /// forward.
    ///
    /// When `frac >= input_rate`, one input sample has been consumed and
    /// `frac` wraps by subtracting `input_rate`.
    frac: u64,
    /// Carry-over from the previous input chunk: the last sample(s) of the
    /// previous call, used for interpolation across chunk boundaries.
    carry: Vec<i16>,
    /// Accumulated output samples that did not yet form a complete Opus
    /// frame. The caller drains this via [`Self::drain_frame`].
    pending: VecDeque<i16>,
}

impl AudioResampler {
    /// Build a resampler for the given device configuration.
    #[must_use]
    pub fn new(input_rate: u32, input_channels: u8) -> Self {
        // One carry sample per input channel so linear interpolation has a
        // "previous" sample at the start of each chunk.
        let carry = vec![0i16; input_channels as usize];
        Self {
            input_rate,
            input_channels,
            frac: 0,
            carry,
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
        let in_frames = samples.len() / in_channels;

        // We'll process one output sample at a time. For each output sample
        // at 48 kHz, we need to find the corresponding position in the input
        // stream. Linear interpolation: output[n] = LERP(input[floor],
        // input[ceil], frac_part).
        let in_rate = self.input_rate as u64;
        let out_rate = TARGET_SAMPLE_RATE as u64;
        let out_channels = TARGET_CHANNELS as usize;

        // Prepend the carry from the previous call so interpolation across
        // the chunk boundary works.
        let mut ring: VecDeque<i16> = VecDeque::with_capacity(in_channels + samples.len());
        ring.extend(self.carry.iter().copied());
        ring.extend(samples.iter().copied());

        loop {
            // Position in the (carry + input) stream, in input-rate units
            // per _channel_ (not per interleaved frame).
            let pos = self.frac / in_rate;
            let frac_part = (self.frac % in_rate) as f64 / in_rate as f64;

            let base_idx = pos as usize * in_channels;
            let next_idx = base_idx + in_channels;

            if next_idx + in_channels - 1 >= ring.len() {
                break; // Not enough samples to produce another output.
            }

            // For each output channel (always 1 for mono), sample from
            // the corresponding input channel, downmixing if needed.
            let mut out_sample = 0.0_f64;
            if in_channels == 1 {
                let a = f64::from(ring[base_idx]);
                let b = f64::from(ring[next_idx]);
                out_sample = a + (b - a) * frac_part;
            } else {
                // Stereo → mono downmix: average L and R, then interpolate.
                for ch in 0..in_channels {
                    let a = f64::from(ring[base_idx + ch]);
                    let b = f64::from(ring[next_idx + ch]);
                    out_sample += (a + (b - a) * frac_part) / in_channels as f64;
                }
            }

            self.pending
                .push_back((out_sample.round() as i16).clamp(i16::MIN, i16::MAX));

            self.frac += out_rate;
            if self.frac >= in_rate {
                let consumed = (self.frac / in_rate) as usize;
                // Save the last input frame(s) for interpolation across
                // the next chunk boundary.
                let keep = (consumed.saturating_sub(1) * in_channels).min(ring.len());
                // Actually, we need the sample right before the current
                // frac position. The last input frame we haven't consumed
                // yet provides the "previous" for the next output.
                self.frac %= in_rate;
                // Keep the last input frame as carry.
                let carry_start = (pos as usize).min(in_frames.saturating_sub(1)) * in_channels;
                if carry_start + in_channels <= samples.len() {
                    self.carry
                        .copy_from_slice(&samples[carry_start..carry_start + in_channels]);
                }
                // Drop consumed portion from `ring`.
                let drain_count = consumed * in_channels;
                ring.drain(..drain_count.min(ring.len()));
            } else {
                // Not enough to consume a full input sample yet.
                // `ring` stays as-is; we'll use it next iteration.
                break;
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
        self.frac = 0;
        self.carry.fill(0);
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
        assert_eq!(rs.frac, 0);
    }

    #[test]
    fn downmix_stereo_to_mono_averages_channels() {
        // Two identical sine waves → mono should match one channel exactly.
        let mut rs = AudioResampler::new(48_000, 2);
        let stereo: Vec<i16> = (0..1920)
            .flat_map(|i| {
                let s = (i as i16 % 100);
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
