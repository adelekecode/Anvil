//! Opus encoder and decoder (§27).
//!
//! Settings that matter for this system, and why:
//!
//! | Setting | Value | Reason |
//! |---|---|---|
//! | Application | VOIP | biases toward intelligibility over musical fidelity |
//! | Sample rate | 48 kHz | Opus's native rate; anything else costs a resample |
//! | Channels | 1 | stereo doubles the bitrate for no benefit on a phone |
//! | Frame | 20 ms | the standard latency/overhead trade-off |
//! | Bitrate | 24 kbps | transparent for speech; tune in Phase 7 |
//! | In-band FEC | on | recovers isolated losses cheaply |
//! | DTX | off | Anvil does VAD itself, above the codec |
//!
//! Two of those deserve more than a table row.
//!
//! **FEC costs nothing when there is no loss** — Opus only adds redundancy when
//! it is told loss is occurring, so `set_packet_loss_perc` should be driven from
//! measured path loss rather than pinned to a constant. Pinning it high wastes
//! bandwidth on a clean LAN; pinning it low wastes the feature.
//!
//! **DTX is off deliberately.** Opus can do its own discontinuous transmission,
//! but Anvil's VAD decides at the packet layer, where it also drives the
//! talkspurt flag, the "who is speaking" event and the relay's forwarding load.
//! Two independent silence detectors would fight each other and produce
//! clipping neither one is responsible for.
//!
//! ## Phase status
//!
//! PHASE1. Behind the `opus` cargo feature because `audiopus` builds libopus
//! from C, which needs a toolchain the bare scaffold should not require.

#[cfg(feature = "opus")]
mod imp {
    use crate::audio::codec::{Decoder, Encoder};
    use crate::audio::frame::PcmFrame;
    use crate::{AudioConfig, Result};

    /// Opus encoder.
    #[derive(Debug)]
    pub struct OpusEncoder {
        #[allow(dead_code)]
        config: AudioConfig,
    }

    impl OpusEncoder {
        /// Build an encoder for the configured format.
        pub fn new(_config: &AudioConfig) -> Result<Self> {
            Err(crate::Error::NotImplemented("audio::opus::OpusEncoder::new (Phase 1)"))
        }
    }

    impl Encoder for OpusEncoder {
        fn encode(&mut self, _frame: &PcmFrame) -> Result<Vec<u8>> {
            Err(crate::Error::NotImplemented("audio::opus::encode (Phase 1)"))
        }
        fn set_bitrate(&mut self, _bps: u32) -> Result<()> {
            Err(crate::Error::NotImplemented("audio::opus::set_bitrate (Phase 1)"))
        }
        fn set_max_payload(&mut self, _bytes: usize) -> Result<()> {
            Err(crate::Error::NotImplemented("audio::opus::set_max_payload (Phase 1)"))
        }
    }

    /// Opus decoder.
    #[derive(Debug)]
    pub struct OpusDecoder {
        #[allow(dead_code)]
        config: AudioConfig,
    }

    impl Decoder for OpusDecoder {
        fn decode(&mut self, _payload: &[u8]) -> Result<PcmFrame> {
            Err(crate::Error::NotImplemented("audio::opus::decode (Phase 1)"))
        }
        fn conceal(&mut self) -> Result<PcmFrame> {
            Err(crate::Error::NotImplemented("audio::opus::conceal (Phase 1)"))
        }
    }
}

#[cfg(feature = "opus")]
pub use imp::{OpusDecoder, OpusEncoder};

/// Opus application mode Anvil uses.
pub const APPLICATION: &str = "voip";

/// Frame durations Opus accepts, in milliseconds.
///
/// Anvil uses 20 ms. 10 ms halves frame delay but nearly doubles per-packet
/// overhead, which matters most in exactly the case where latency matters —
/// a relay fanning out to several peers over one radio.
pub const VALID_FRAME_MS: &[u32] = &[2, 5, 10, 20, 40, 60];

/// Whether a frame duration is one Opus supports.
#[must_use]
pub fn is_valid_frame_duration(millis: u32) -> bool {
    VALID_FRAME_MS.contains(&millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_frame_duration_is_one_opus_supports() {
        let config = crate::AudioConfig::default();
        assert!(is_valid_frame_duration(config.frame_duration.as_millis() as u32));
    }

    #[test]
    fn rejects_arbitrary_frame_durations() {
        assert!(!is_valid_frame_duration(15));
        assert!(!is_valid_frame_duration(0));
    }
}
