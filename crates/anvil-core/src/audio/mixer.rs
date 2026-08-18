//! Local audio mixing (§72, §73).
//!
//! Every participant mixes locally, from streams they decrypted themselves.
//! The relay never mixes — it cannot, because mixing requires plaintext, and a
//! relay with plaintext is not a relay, it is a server that can hear the room
//! (§73).
//!
//! ```text
//!   Alice ─┐
//!   Bob   ─┼─► sum ─► limit ─► speaker
//!   Chris ─┘
//! ```
//!
//! ## Clipping
//!
//! Three people talking at once easily exceeds `i16` range, and naive wrapping
//! addition turns overflow into a loud crack — the worst possible artefact, in
//! headphones, at whatever volume the user chose. So the mix saturates rather
//! than wraps, and above a threshold it applies soft-knee compression instead
//! of hard clipping, which is audible as slight loudness reduction rather than
//! distortion.

use crate::audio::frame::PcmFrame;
use crate::{MediaTimestamp, PeerId};

/// Level above which soft limiting begins, as a fraction of full scale.
const KNEE: f32 = 0.7;

/// Mixes decoded streams into one output frame.
#[derive(Debug)]
pub struct Mixer {
    accumulator: Vec<f32>,
    samples_per_frame: usize,
    sample_rate_hz: u32,
    channels: u8,
    /// Whether anything was contributed this frame.
    contributed: bool,
}

impl Mixer {
    /// Build a mixer for the configured format.
    #[must_use]
    pub fn new(config: &crate::AudioConfig) -> Self {
        let samples_per_frame = super::frame::samples_per_frame(config);
        Self {
            accumulator: vec![0.0; samples_per_frame],
            samples_per_frame,
            sample_rate_hz: config.sample_rate_hz,
            channels: config.channels,
            contributed: false,
        }
    }

    /// Start a new output frame.
    pub fn begin(&mut self) {
        self.accumulator.fill(0.0);
        self.contributed = false;
    }

    /// Add one participant's decoded frame.
    ///
    /// Frames of the wrong length are mixed as far as they go rather than
    /// rejected — a short frame from one participant should not silence
    /// everybody.
    pub fn add(&mut self, _from: PeerId, frame: &PcmFrame) {
        for (acc, sample) in self.accumulator.iter_mut().zip(frame.samples.iter()) {
            *acc += f32::from(*sample);
        }
        self.contributed = true;
    }

    /// Produce the mixed frame.
    #[must_use]
    pub fn finish(&self, timestamp: MediaTimestamp) -> PcmFrame {
        let samples = self.accumulator.iter().map(|s| limit(*s)).collect::<Vec<i16>>();

        PcmFrame::new(samples, self.sample_rate_hz, self.channels, timestamp)
    }

    /// Whether any participant contributed to this frame. When false the
    /// caller can skip playback entirely and save the wakeup.
    #[must_use]
    pub const fn has_audio(&self) -> bool {
        self.contributed
    }

    /// Output frame length in samples.
    #[must_use]
    pub const fn frame_len(&self) -> usize {
        self.samples_per_frame
    }
}

/// Soft limiter. Linear below the knee, compressed above it, saturating at full
/// scale — never wrapping.
fn limit(sample: f32) -> i16 {
    let scale = f32::from(i16::MAX);
    let normalised = sample / scale;
    let magnitude = normalised.abs();

    let limited = if magnitude <= KNEE {
        normalised
    } else {
        // Asymptotically approaches 1.0; compresses the overshoot.
        let over = magnitude - KNEE;
        let compressed = KNEE + (1.0 - KNEE) * (over / (over + (1.0 - KNEE)));
        compressed * normalised.signum()
    };

    (limited * scale).clamp(-scale, scale) as i16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioConfig;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn frame(level: i16, config: &AudioConfig) -> PcmFrame {
        PcmFrame::new(
            vec![level; super::super::frame::samples_per_frame(config)],
            config.sample_rate_hz,
            config.channels,
            MediaTimestamp(0),
        )
    }

    #[test]
    fn mixing_nothing_produces_silence() {
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);
        mixer.begin();

        assert!(!mixer.has_audio());
        let out = mixer.finish(MediaTimestamp(0));
        assert!(out.samples.iter().all(|s| *s == 0));
        out.validate(&config).unwrap();
    }

    #[test]
    fn a_single_quiet_stream_passes_through_unchanged() {
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);
        mixer.begin();
        mixer.add(peer(1), &frame(1_000, &config));

        let out = mixer.finish(MediaTimestamp(0));
        assert!(mixer.has_audio());
        // Below the knee: no compression.
        assert!(out.samples.iter().all(|s| (*s - 1_000).abs() <= 1), "{:?}", &out.samples[..4]);
    }

    #[test]
    fn three_loud_streams_do_not_wrap_into_a_crack() {
        // The bug this exists to prevent: naive i16 addition overflowing and
        // producing a full-scale sign flip in someone's headphones.
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);
        mixer.begin();
        for p in 1..=3u8 {
            mixer.add(peer(p), &frame(20_000, &config));
        }

        let out = mixer.finish(MediaTimestamp(0));
        assert!(
            out.samples.iter().all(|s| *s > 0),
            "mixed audio changed sign — that is an overflow, and it is audible"
        );
    }

    #[test]
    fn extreme_input_saturates_rather_than_overflowing() {
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);
        mixer.begin();
        for p in 1..=8u8 {
            mixer.add(peer(p), &frame(i16::MAX, &config));
        }

        let out = mixer.finish(MediaTimestamp(0));
        assert!(out.samples.iter().all(|s| *s > 30_000));
    }

    #[test]
    fn negative_extremes_saturate_too() {
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);
        mixer.begin();
        for p in 1..=8u8 {
            mixer.add(peer(p), &frame(i16::MIN + 1, &config));
        }

        let out = mixer.finish(MediaTimestamp(0));
        assert!(out.samples.iter().all(|s| *s < -30_000));
    }

    #[test]
    fn begin_clears_the_previous_frame() {
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);

        mixer.begin();
        mixer.add(peer(1), &frame(5_000, &config));
        let _ = mixer.finish(MediaTimestamp(0));

        mixer.begin();
        let out = mixer.finish(MediaTimestamp(1));
        assert!(out.samples.iter().all(|s| *s == 0), "audio leaked between frames");
    }

    #[test]
    fn a_short_frame_does_not_silence_the_room() {
        let config = AudioConfig::default();
        let mut mixer = Mixer::new(&config);
        mixer.begin();

        let short = PcmFrame::new(
            vec![3_000; 100],
            config.sample_rate_hz,
            config.channels,
            MediaTimestamp(0),
        );
        mixer.add(peer(1), &short);
        mixer.add(peer(2), &frame(3_000, &config));

        let out = mixer.finish(MediaTimestamp(0));
        assert_eq!(out.samples.len(), mixer.frame_len());
        assert!(out.samples[0] > 5_000, "both streams should be present at the start");
        assert!(out.samples[500] > 2_000, "the full-length stream should continue");
    }
}
