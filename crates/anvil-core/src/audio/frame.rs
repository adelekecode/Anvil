//! Audio frames.

use crate::{AudioError, MediaTimestamp, Result, SeqNum};

/// A block of uncompressed audio.
///
/// Interleaved `i16` samples. `i16` rather than `f32` because it is what both
/// platform capture APIs hand over natively and what Opus takes, so the common
/// path involves no conversion at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PcmFrame {
    /// Interleaved samples.
    pub samples: Vec<i16>,
    /// Sample rate.
    pub sample_rate_hz: u32,
    /// Channel count.
    pub channels: u8,
    /// Sender media clock at the frame's first sample.
    pub timestamp: MediaTimestamp,
}

impl PcmFrame {
    /// Build a frame.
    #[must_use]
    pub fn new(
        samples: Vec<i16>,
        sample_rate_hz: u32,
        channels: u8,
        timestamp: MediaTimestamp,
    ) -> Self {
        Self { samples, sample_rate_hz, channels, timestamp }
    }

    /// Silence of the given duration, used for concealment and for filling a
    /// starved jitter buffer.
    #[must_use]
    pub fn silence(config: &crate::AudioConfig, timestamp: MediaTimestamp) -> Self {
        Self {
            samples: vec![0; samples_per_frame(config)],
            sample_rate_hz: config.sample_rate_hz,
            channels: config.channels,
            timestamp,
        }
    }

    /// Samples per channel.
    #[must_use]
    pub fn samples_per_channel(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    /// Frame duration.
    #[must_use]
    pub fn duration(&self) -> core::time::Duration {
        core::time::Duration::from_micros(
            (self.samples_per_channel() as u64 * 1_000_000) / self.sample_rate_hz.max(1) as u64,
        )
    }

    /// Check the frame matches the negotiated format.
    ///
    /// Worth doing at the boundary: a platform adapter quietly handing over
    /// 44.1 kHz stereo when 48 kHz mono was requested produces audio that is
    /// merely *wrong* rather than absent, which is far harder to diagnose from
    /// a bug report.
    pub fn validate(&self, config: &crate::AudioConfig) -> Result<()> {
        if self.sample_rate_hz != config.sample_rate_hz {
            return Err(AudioError::FrameFormat("sample rate mismatch").into());
        }
        if self.channels != config.channels {
            return Err(AudioError::FrameFormat("channel count mismatch").into());
        }
        if self.samples.len() != samples_per_frame(config) {
            return Err(AudioError::FrameFormat("unexpected frame length").into());
        }
        Ok(())
    }
}

/// An encoded, not-yet-encrypted Opus frame with its sequencing metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Opus payload.
    pub payload: Vec<u8>,
    /// Sequence number.
    pub sequence: SeqNum,
    /// Media timestamp.
    pub timestamp: MediaTimestamp,
    /// First frame after VAD silence.
    pub talkspurt_start: bool,
}

/// Total samples in one frame at the configured rate, duration and channels.
#[must_use]
pub fn samples_per_frame(config: &crate::AudioConfig) -> usize {
    let per_channel =
        (config.sample_rate_hz as u128 * config.frame_duration.as_micros()) / 1_000_000;
    per_channel as usize * config.channels as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioConfig;

    #[test]
    fn default_config_gives_a_960_sample_frame() {
        // 20ms @ 48kHz mono.
        assert_eq!(samples_per_frame(&AudioConfig::default()), 960);
    }

    #[test]
    fn ten_millisecond_frames_are_half_the_size() {
        let config = AudioConfig {
            frame_duration: core::time::Duration::from_millis(10),
            ..AudioConfig::default()
        };
        assert_eq!(samples_per_frame(&config), 480);
    }

    #[test]
    fn silence_matches_the_configured_format() {
        let config = AudioConfig::default();
        let frame = PcmFrame::silence(&config, MediaTimestamp(0));

        frame.validate(&config).unwrap();
        assert_eq!(frame.duration(), config.frame_duration);
        assert!(frame.samples.iter().all(|s| *s == 0));
    }

    #[test]
    fn validation_catches_a_misconfigured_adapter() {
        let config = AudioConfig::default();

        let wrong_rate = PcmFrame::new(vec![0; 960], 44_100, 1, MediaTimestamp(0));
        assert!(wrong_rate.validate(&config).is_err());

        let wrong_channels = PcmFrame::new(vec![0; 960], 48_000, 2, MediaTimestamp(0));
        assert!(wrong_channels.validate(&config).is_err());

        let wrong_length = PcmFrame::new(vec![0; 512], 48_000, 1, MediaTimestamp(0));
        assert!(wrong_length.validate(&config).is_err());
    }
}
