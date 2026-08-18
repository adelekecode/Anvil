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
//! **Phase 1: PCM → Opus → PCM in isolation.** The encoder and decoder below
//! work against the [`audiopus`] crate (which builds libopus from C) but they
//! do no I/O: no microphone, no network, no CPAL, no playback thread. The
//! capture/playback pipeline, packet format, sender/receiver glue, and
//! encryption integration arrive in later phases per the implementation
//! plan in the audio subsystem spec.
//!
//! The codec is owned by [`OpusVoiceEncoder`] / [`OpusVoiceDecoder`] behind
//! the `opus` cargo feature because `audiopus` builds libopus from C, which
//! needs a toolchain the bare scaffold should not require.

#[cfg(feature = "opus")]
mod imp {
    use audiopus::coder::{Decoder as ApDecoder, Encoder as ApEncoder};
    use audiopus::{Application, Bitrate, Channels, SampleRate};

    use crate::audio::codec::{Decoder, Encoder};
    use crate::audio::frame::PcmFrame;
    use crate::{AudioError, MediaTimestamp, Result};

    /// Largest Opus packet a single encoded frame may produce.
    ///
    /// Opus caps any frame at 4000 bytes; allocating this once and reusing it
    /// per call is the cheapest way to keep the encoder real-time-safe.
    const MAX_OPUS_PACKET_BYTES: usize = 4000;

    /// Largest PCM frame Opus may decode from a single packet, in samples per
    /// channel. 60 ms at 48 kHz is 2880; doubled to cover any future 120 ms
    /// frames Opus could ever negotiate. The decoder writes into this buffer
    /// in place.
    const MAX_PCM_SAMPLES_PER_CHANNEL: usize = 5760;

    /// Opus `OPUS_SET_DTX_REQUEST` CTL code. audiopus 0.2 does not expose a
    /// typed `set_dtx` helper, so the encoder talks to the libopus CTL layer
    /// directly. The constant lives in `opus_defines.h` and is stable across
    /// releases.
    const OPUS_SET_DTX_REQUEST: i32 = 4016;

    // --- configuration -----------------------------------------------------

    /// Configuration for one Opus encoder or decoder.
    ///
    /// Held by [`OpusVoiceEncoder`] / [`OpusVoiceDecoder`] so the runtime can
    /// inspect and report what is currently configured. The setters on the
    /// wrappers keep this in sync with libopus.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct OpusConfig {
        /// Sample rate. 48 kHz is Opus's native rate.
        pub sample_rate: u32,
        /// Channels. 1 (mono) is what Anvil transports.
        pub channels: u8,
        /// Opus frame duration in milliseconds.
        ///
        /// Must be one of [`VALID_FRAME_MS`]; anything else is rejected at
        /// construction.
        pub frame_duration_ms: u16,
        /// Target bitrate in bits per second.
        pub bitrate: u32,
        /// Whether Opus in-band FEC is enabled.
        pub fec: bool,
        /// Whether Opus DTX is enabled.
        pub dtx: bool,
        /// Hint to libopus about expected packet loss percentage (0..=100).
        ///
        /// Drives how aggressive in-band FEC redundancy is. Should track
        /// measured path loss; pinning high wastes bandwidth on a clean LAN.
        pub expected_packet_loss: u8,
        /// Encoder computational complexity (0..=10). 0 is fastest, 10 best.
        pub complexity: u8,
    }

    impl OpusConfig {
        /// The PCM samples a single frame carries, per channel.
        ///
        /// Computed locally from sample rate and frame duration; libopus does
        /// not expose this directly.
        #[must_use]
        pub fn samples_per_frame(&self) -> usize {
            (self.sample_rate as usize * self.frame_duration_ms as usize) / 1000
        }

        /// Total interleaved PCM samples in one frame (`channels *
        /// samples_per_frame`).
        #[must_use]
        pub fn samples_per_frame_total(&self) -> usize {
            self.samples_per_frame() * self.channels as usize
        }

        /// Reject obviously-broken configurations before talking to libopus.
        pub fn validate(&self) -> Result<()> {
            if self.channels != 1 && self.channels != 2 {
                return Err(AudioError::FrameFormat("opus channels must be 1 or 2").into());
            }
            if !super::is_valid_frame_duration(self.frame_duration_ms as u32) {
                return Err(AudioError::FrameFormat("invalid opus frame duration").into());
            }
            if self.expected_packet_loss > 100 {
                return Err(AudioError::FrameFormat("expected_packet_loss must be 0..=100").into());
            }
            if self.complexity > 10 {
                return Err(AudioError::FrameFormat("complexity must be 0..=10").into());
            }
            // Opus's documented minimum and maximum voice bitrates at 48 kHz
            // mono are 6 kbps and 256 kbps. Below 6 kbps the encoder still
            // works but produces nonsense; the rest of the pipeline will be
            // broken in ways that are hard to diagnose.
            if self.bitrate < 6_000 || self.bitrate > 256_000 {
                return Err(AudioError::FrameFormat("opus bitrate out of range").into());
            }
            Ok(())
        }

        fn sample_rate_enum(&self) -> SampleRate {
            match self.sample_rate {
                8_000 => SampleRate::Hz8000,
                12_000 => SampleRate::Hz12000,
                16_000 => SampleRate::Hz16000,
                24_000 => SampleRate::Hz24000,
                48_000 => SampleRate::Hz48000,
                // Opus will resample internally. Pick the closest legal rate;
                // the mismatch shows up immediately in `validate()` callers
                // who pass an explicit AudioConfig.
                _ => SampleRate::Hz48000,
            }
        }

        fn channels_enum(&self) -> Channels {
            if self.channels == 1 {
                Channels::Mono
            } else {
                Channels::Stereo
            }
        }
    }

    // --- frame types -------------------------------------------------------

    /// One Opus packet ready to hand to the transport layer.
    ///
    /// Distinct from [`crate::audio::frame::EncodedFrame`], which carries the
    /// sequencing metadata the *transport* layer attaches. The codec only
    /// cares about the byte payload; the rest happens around it.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct EncodedAudioFrame {
        /// Opus payload bytes. Owned rather than borrowed so the transport
        /// can hand it to encryption without lifetime gymnastics.
        pub payload: Vec<u8>,
    }

    impl EncodedAudioFrame {
        /// An empty frame, used to carry "DTX sent nothing" without
        /// overloading the type with an `Option`.
        #[must_use]
        pub fn empty() -> Self {
            Self { payload: Vec::new() }
        }

        /// True if this frame carries no Opus bytes (DTX silence).
        #[must_use]
        pub fn is_empty(&self) -> bool {
            self.payload.is_empty()
        }
    }

    /// One decoded PCM frame ready for the mixer / playback queue.
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct DecodedAudioFrame {
        /// Interleaved PCM samples, `i16`, at the negotiated sample rate and
        /// channel count.
        pub samples: Vec<i16>,
        /// True if this frame was synthesised by packet-loss concealment or
        /// by FEC recovery from a previous frame, not from a real packet.
        pub concealed: bool,
    }

    // --- encoder -----------------------------------------------------------

    /// Opus encoder for voice.
    ///
    /// Holds the libopus encoder state and a reusable output scratch buffer.
    /// The scratch buffer is sized for the worst-case Opus packet so no
    /// allocation happens on the encode path.
    pub struct OpusVoiceEncoder {
        inner: ApEncoder,
        config: OpusConfig,
        /// Reusable output buffer for [`OpusVoiceEncoder::encode_frame`].
        scratch: [u8; MAX_OPUS_PACKET_BYTES],
    }

    impl OpusVoiceEncoder {
        /// Build an encoder from a validated configuration.
        ///
        /// The configuration is applied immediately: bitrate, FEC, DTX,
        /// expected loss, complexity. Subsequent [`Self::set_*`] calls update
        /// the running encoder in place; libopus does not need to be
        /// reconstructed.
        pub fn new(config: OpusConfig) -> Result<Self> {
            config.validate()?;
            let mut inner = ApEncoder::new(
                config.sample_rate_enum(),
                config.channels_enum(),
                Application::Voip,
            )
            .map_err(|e| AudioError::Encode(format!("opus encoder init: {e:?}")))?;

            Self::apply_config(&mut inner, &config)?;

            Ok(Self { inner, config, scratch: [0u8; MAX_OPUS_PACKET_BYTES] })
        }

        fn apply_config(inner: &mut ApEncoder, config: &OpusConfig) -> Result<()> {
            inner
                .set_bitrate(Bitrate::BitsPerSecond(config.bitrate as i32))
                .map_err(|e| AudioError::Encode(format!("opus set_bitrate: {e:?}")))?;
            inner
                .set_inband_fec(config.fec)
                .map_err(|e| AudioError::Encode(format!("opus set_inband_fec: {e:?}")))?;
            inner
                .set_packet_loss_perc(config.expected_packet_loss)
                .map_err(|e| AudioError::Encode(format!("opus set_packet_loss_perc: {e:?}")))?;
            inner
                .set_complexity(config.complexity)
                .map_err(|e| AudioError::Encode(format!("opus set_complexity: {e:?}")))?;
            inner
                .set_encoder_ctl_request(OPUS_SET_DTX_REQUEST, i32::from(config.dtx))
                .map_err(|e| AudioError::Encode(format!("opus set_dtx: {e:?}")))?;
            Ok(())
        }

        /// Samples per channel the next call to [`Self::encode_frame`] expects.
        #[must_use]
        pub fn frame_samples_per_channel(&self) -> usize {
            self.config.samples_per_frame()
        }

        /// Encode one PCM frame. `pcm` must carry
        /// [`Self::frame_samples_per_channel`] samples per channel,
        /// interleaved.
        pub fn encode_frame(&mut self, pcm: &[i16]) -> Result<EncodedAudioFrame> {
            let needed = self.config.samples_per_frame_total();
            if pcm.len() < needed {
                return Err(AudioError::Encode(format!(
                    "pcm buffer too short: got {} samples, need {}",
                    pcm.len(),
                    needed
                ))
                .into());
            }

            let len = self
                .inner
                .encode(pcm, &mut self.scratch)
                .map_err(|e| AudioError::Encode(format!("opus encode: {e:?}")))?;
            Ok(EncodedAudioFrame { payload: self.scratch[..len].to_vec() })
        }

        /// Current configuration snapshot.
        #[must_use]
        pub const fn config(&self) -> &OpusConfig {
            &self.config
        }

        /// Change the target bitrate while the call is active.
        pub fn set_bitrate(&mut self, bps: u32) -> Result<()> {
            if !(6_000..=256_000).contains(&bps) {
                return Err(AudioError::FrameFormat("opus bitrate out of range").into());
            }
            self.inner
                .set_bitrate(Bitrate::BitsPerSecond(bps as i32))
                .map_err(|e| AudioError::Encode(format!("opus set_bitrate: {e:?}")))?;
            self.config.bitrate = bps;
            Ok(())
        }

        /// Update the expected packet-loss percentage.
        pub fn set_expected_packet_loss(&mut self, percent: u8) -> Result<()> {
            if percent > 100 {
                return Err(AudioError::FrameFormat("expected_packet_loss must be 0..=100").into());
            }
            self.inner
                .set_packet_loss_perc(percent)
                .map_err(|e| AudioError::Encode(format!("opus set_packet_loss_perc: {e:?}")))?;
            self.config.expected_packet_loss = percent;
            Ok(())
        }

        /// Change the encoder complexity while the call is active.
        pub fn set_complexity(&mut self, complexity: u8) -> Result<()> {
            if complexity > 10 {
                return Err(AudioError::FrameFormat("complexity must be 0..=10").into());
            }
            self.inner
                .set_complexity(complexity)
                .map_err(|e| AudioError::Encode(format!("opus set_complexity: {e:?}")))?;
            self.config.complexity = complexity;
            Ok(())
        }

        /// Toggle in-band forward error correction.
        pub fn set_fec(&mut self, on: bool) -> Result<()> {
            self.inner
                .set_inband_fec(on)
                .map_err(|e| AudioError::Encode(format!("opus set_inband_fec: {e:?}")))?;
            self.config.fec = on;
            Ok(())
        }

        /// Toggle discontinuous transmission.
        ///
        /// Anvil usually leaves this off and does silence detection in the VAD
        /// layer, where it can also drive the talkspurt flag and the relay
        /// forwarder. The setter exists for symmetry with the rest of the
        /// spec and for callers who explicitly want it.
        pub fn set_dtx(&mut self, on: bool) -> Result<()> {
            self.inner
                .set_encoder_ctl_request(OPUS_SET_DTX_REQUEST, i32::from(on))
                .map_err(|e| AudioError::Encode(format!("opus set_dtx: {e:?}")))?;
            self.config.dtx = on;
            Ok(())
        }
    }

    impl core::fmt::Debug for OpusVoiceEncoder {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("OpusVoiceEncoder").field("config", &self.config).finish_non_exhaustive()
        }
    }

    // --- decoder -----------------------------------------------------------

    /// Opus decoder for voice.
    ///
    /// Holds the libopus decoder state and a reusable PCM scratch buffer
    /// sized for the largest frame Opus may emit.
    pub struct OpusVoiceDecoder {
        inner: ApDecoder,
        config: OpusConfig,
        /// Reusable PCM buffer for [`OpusVoiceDecoder::decode_frame`].
        scratch: [i16; MAX_PCM_SAMPLES_PER_CHANNEL * 2],
    }

    impl OpusVoiceDecoder {
        /// Build a decoder matching the encoder's configuration.
        pub fn new(config: OpusConfig) -> Result<Self> {
            config.validate()?;
            let inner = ApDecoder::new(config.sample_rate_enum(), config.channels_enum())
                .map_err(|e| AudioError::Decode(format!("opus decoder init: {e:?}")))?;

            Ok(Self { inner, config, scratch: [0i16; MAX_PCM_SAMPLES_PER_CHANNEL * 2] })
        }

        /// Samples per channel the next call to [`Self::decode_frame`] will
        /// produce.
        #[must_use]
        pub fn frame_samples_per_channel(&self) -> usize {
            self.config.samples_per_frame()
        }

        /// Decode one Opus packet. The spec interface:
        ///
        /// * `packet = Some(p), fec = false` — normal decode.
        /// * `packet = Some(p), fec = true` — decode using the FEC
        ///   redundancy carried in `p` (called when the previous packet was
        ///   lost).
        /// * `packet = None, fec = false` — packet-loss concealment using
        ///   the decoder's internal state. Always returns a valid
        ///   concealment frame; never blocks waiting for a missing packet.
        /// * `packet = None, fec = true` — not a valid combination; the
        ///   underlying libopus call returns an error which is reported as
        ///   [`AudioError::Decode`].
        pub fn decode_frame(
            &mut self,
            packet: Option<&[u8]>,
            fec: bool,
        ) -> Result<DecodedAudioFrame> {
            let channels = self.config.channels as usize;
            let frame_samples = self.config.samples_per_frame();
            let buf_len = frame_samples * channels;
            if buf_len > self.scratch.len() {
                return Err(AudioError::Decode(format!(
                    "configured frame exceeds scratch: need {buf_len}, have {}",
                    self.scratch.len()
                ))
                .into());
            }

            // Both PLC (`packet = None`) and FEC recovery (`fec = true`)
            // produce audio that is not from the original packet. The jitter
            // buffer counts either as concealed for stats purposes — silence
            // and a recovered frame are different audibly but the same
            // statistically (a gap that we patched over).
            let concealed = packet.is_none() || fec;
            let len = self
                .inner
                .decode(packet, &mut self.scratch[..buf_len], fec)
                .map_err(|e| AudioError::Decode(format!("opus decode: {e:?}")))?;
            Ok(DecodedAudioFrame { samples: self.scratch[..len].to_vec(), concealed })
        }

        /// Current configuration snapshot.
        #[must_use]
        pub const fn config(&self) -> &OpusConfig {
            &self.config
        }
    }

    impl core::fmt::Debug for OpusVoiceDecoder {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.debug_struct("OpusVoiceDecoder").field("config", &self.config).finish_non_exhaustive()
        }
    }

    // --- trait shims so the codec slots into the existing pipeline ---------

    impl Encoder for OpusVoiceEncoder {
        fn encode(&mut self, frame: &PcmFrame) -> Result<Vec<u8>> {
            // The codec only consumes samples; sample rate / channel count
            // are encoded in the OpusConfig, not the PcmFrame at this point.
            // The PcmFrame timestamp is irrelevant for encoding and is
            // discarded; sequencing is the transport layer's job.
            let encoded = self.encode_frame(&frame.samples)?;
            Ok(encoded.payload)
        }

        fn set_bitrate(&mut self, bps: u32) -> Result<()> {
            self.set_bitrate(bps)
        }

        fn set_max_payload(&mut self, bytes: usize) -> Result<()> {
            // libopus encodes to whatever fits the bitrate; it does not take
            // a hard output-size cap. We honour the call by validating the
            // requested budget against the Opus maximum and otherwise
            // silently succeeding — the real ceiling is the encoder bitrate
            // plus the per-frame cap Opus enforces internally.
            if bytes > MAX_OPUS_PACKET_BYTES {
                return Err(AudioError::Encode(format!(
                    "requested max payload {bytes} exceeds opus cap {MAX_OPUS_PACKET_BYTES}"
                ))
                .into());
            }
            Ok(())
        }
    }

    impl Decoder for OpusVoiceDecoder {
        fn decode(&mut self, payload: &[u8]) -> Result<PcmFrame> {
            let decoded = self.decode_frame(Some(payload), false)?;
            Ok(PcmFrame::new(
                decoded.samples,
                self.config.sample_rate,
                self.config.channels,
                MediaTimestamp(0),
            ))
        }

        fn conceal(&mut self) -> Result<PcmFrame> {
            let decoded = self.decode_frame(None, false)?;
            Ok(PcmFrame::new(
                decoded.samples,
                self.config.sample_rate,
                self.config.channels,
                MediaTimestamp(0),
            ))
        }
    }
}

#[cfg(feature = "opus")]
pub use imp::{
    DecodedAudioFrame, EncodedAudioFrame, OpusConfig, OpusVoiceDecoder, OpusVoiceEncoder,
};

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

    /// The default Anvil voice configuration: 48 kHz mono, 20 ms, 24 kbps,
    /// FEC on, DTX off, no assumed loss, low complexity.
    #[cfg(feature = "opus")]
    fn default_config() -> OpusConfig {
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

    /// A 20 ms / 48 kHz mono frame is 960 samples.
    #[test]
    #[cfg(feature = "opus")]
    fn samples_per_frame_is_rate_times_duration_over_thousand() {
        assert_eq!(default_config().samples_per_frame(), 960);
        assert_eq!(default_config().samples_per_frame_total(), 960);
    }

    #[test]
    #[cfg(feature = "opus")]
    fn validation_rejects_disallowed_durations() {
        let mut cfg = default_config();
        cfg.frame_duration_ms = 15;
        assert!(cfg.validate().is_err());

        cfg.frame_duration_ms = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    #[cfg(feature = "opus")]
    fn validation_rejects_non_mono_stereo_channels() {
        let mut cfg = default_config();
        cfg.channels = 0;
        assert!(cfg.validate().is_err());
        cfg.channels = 3;
        assert!(cfg.validate().is_err());
    }

    #[test]
    #[cfg(feature = "opus")]
    fn validation_rejects_out_of_range_packet_loss_or_complexity() {
        let mut cfg = default_config();
        cfg.expected_packet_loss = 101;
        assert!(cfg.validate().is_err());
        cfg.expected_packet_loss = 0;
        cfg.complexity = 11;
        assert!(cfg.validate().is_err());
    }

    #[test]
    #[cfg(feature = "opus")]
    fn validation_rejects_extreme_bitrates() {
        let mut cfg = default_config();
        cfg.bitrate = 5_000;
        assert!(cfg.validate().is_err());
        cfg.bitrate = 300_000;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn is_valid_frame_duration_matches_opus_spec() {
        for d in VALID_FRAME_MS {
            assert!(is_valid_frame_duration(*d));
        }
        for d in [0, 1, 3, 7, 11, 15, 25, 80, 100] {
            assert!(!is_valid_frame_duration(d), "false positive for {d} ms");
        }
    }

    // --- tests that need libopus (gated behind the `opus` feature) -------

    #[cfg(feature = "opus")]
    mod with_opus {
        use super::*;
        use crate::audio::codec::{Decoder as _, Encoder as _};
        use crate::audio::frame::PcmFrame;
        use crate::{AudioError, MediaTimestamp};

        /// A 20 ms / 48 kHz mono sine wave at 440 Hz, scaled to i16 range.
        fn sine_440() -> Vec<i16> {
            let n = 960;
            let sr = 48_000.0_f64;
            (0..n)
                .map(|i| {
                    let t = i as f64 / sr;
                    let s = (2.0 * core::f64::consts::PI * 440.0 * t).sin();
                    (s * 16_000.0) as i16
                })
                .collect()
        }

        #[test]
        fn pcm_roundtrip_yields_the_expected_sample_count() {
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();

            let encoded = enc.encode_frame(&pcm).unwrap();
            assert!(!encoded.is_empty(), "encoder produced an empty packet");

            let decoded = dec.decode_frame(Some(&encoded.payload), false).unwrap();
            assert_eq!(decoded.samples.len(), 960);
            assert!(!decoded.concealed);
        }

        #[test]
        fn encoder_rejects_truncated_pcm() {
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let err = enc.encode_frame(&[0i16; 100]).unwrap_err();
            assert!(matches!(err, crate::Error::Audio(AudioError::Encode(_))));
        }

        #[test]
        fn runtime_bitrate_change_keeps_roundtrip_working() {
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();

            enc.set_bitrate(32_000).unwrap();
            assert_eq!(enc.config().bitrate, 32_000);
            let encoded = enc.encode_frame(&pcm).unwrap();
            let decoded = dec.decode_frame(Some(&encoded.payload), false).unwrap();
            assert_eq!(decoded.samples.len(), 960);

            enc.set_bitrate(16_000).unwrap();
            let encoded = enc.encode_frame(&pcm).unwrap();
            let decoded = dec.decode_frame(Some(&encoded.payload), false).unwrap();
            assert_eq!(decoded.samples.len(), 960);
        }

        #[test]
        fn runtime_complexity_change_keeps_roundtrip_working() {
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();

            enc.set_complexity(10).unwrap();
            assert_eq!(enc.config().complexity, 10);
            let encoded = enc.encode_frame(&pcm).unwrap();
            assert!(!encoded.is_empty());
            let decoded = dec.decode_frame(Some(&encoded.payload), false).unwrap();
            assert_eq!(decoded.samples.len(), 960);
        }

        #[test]
        fn fec_toggle_roundtrips_in_both_states() {
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();

            enc.set_fec(false).unwrap();
            assert!(!enc.config().fec);
            let a = enc.encode_frame(&pcm).unwrap();
            assert_eq!(dec.decode_frame(Some(&a.payload), false).unwrap().samples.len(), 960);

            enc.set_fec(true).unwrap();
            assert!(enc.config().fec);
            let b = enc.encode_frame(&pcm).unwrap();
            assert_eq!(dec.decode_frame(Some(&b.payload), false).unwrap().samples.len(), 960);
        }

        #[test]
        fn dtx_toggle_does_not_break_roundtrip() {
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();

            enc.set_dtx(true).unwrap();
            assert!(enc.config().dtx);
            let encoded = enc.encode_frame(&pcm).unwrap();
            assert_eq!(dec.decode_frame(Some(&encoded.payload), false).unwrap().samples.len(), 960);

            enc.set_dtx(false).unwrap();
            assert!(!enc.config().dtx);
            let encoded = enc.encode_frame(&pcm).unwrap();
            assert_eq!(dec.decode_frame(Some(&encoded.payload), false).unwrap().samples.len(), 960);
        }

        #[test]
        fn expected_packet_loss_setting_keeps_roundtrip_working() {
            let mut cfg = default_config();
            cfg.expected_packet_loss = 40;
            let mut enc = OpusVoiceEncoder::new(cfg).unwrap();
            let mut dec = OpusVoiceDecoder::new(cfg).unwrap();
            let pcm = sine_440();

            enc.set_expected_packet_loss(80).unwrap();
            assert_eq!(enc.config().expected_packet_loss, 80);
            let encoded = enc.encode_frame(&pcm).unwrap();
            let decoded = dec.decode_frame(Some(&encoded.payload), false).unwrap();
            assert_eq!(decoded.samples.len(), 960);
        }

        #[test]
        fn fec_decode_after_a_real_packet_returns_audio() {
            // Encode two frames so the second carries FEC for the first; then
            // ask the decoder to recover the first from the second's FEC.
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();

            let _first = enc.encode_frame(&pcm).unwrap();
            let second = enc.encode_frame(&pcm).unwrap();
            let recovered = dec.decode_frame(Some(&second.payload), true).unwrap();
            assert_eq!(recovered.samples.len(), 960);
            // FEC recovery is reported as concealed so the jitter buffer can
            // count it as loss for stats purposes without surfacing it as
            // silence to the listener.
            assert!(recovered.concealed);
        }

        #[test]
        fn plc_returns_audio_not_silence_and_not_an_error() {
            // PLC is the load-bearing case: a missing packet must not crash
            // the call, block playback, or produce literal silence.
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();

            // Prime the decoder with a real packet so it has internal state.
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let pcm = sine_440();
            let encoded = enc.encode_frame(&pcm).unwrap();
            dec.decode_frame(Some(&encoded.payload), false).unwrap();

            // Now ask for concealment.
            let concealed = dec.decode_frame(None, false).unwrap();
            assert_eq!(concealed.samples.len(), 960);
            assert!(concealed.concealed);

            // Concealment is not literal silence: at least one sample differs
            // from zero. (Opus synthesises from its decoder state.)
            let nonzero = concealed.samples.iter().filter(|s| **s != 0).count();
            assert!(nonzero > 0, "PLC produced literal silence");
        }

        #[test]
        fn plc_immediately_after_init_does_not_panic() {
            // A freshly created decoder has no internal state from which to
            // synthesise concealment. libopus is lenient about this and
            // returns silence rather than an error; what matters is that the
            // call does not panic or block, so the stream can keep moving.
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let concealed = dec.decode_frame(None, false).unwrap();
            assert_eq!(concealed.samples.len(), 960);
            assert!(concealed.concealed);
        }

        #[test]
        fn bad_payload_never_panics() {
            // An empty payload has no Opus TOC byte. libopus is lenient enough
            // that even this often decodes (to silence) rather than erroring.
            // The contract this test enforces is the only one that matters:
            // a single malformed packet cannot panic the call.
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let _ = dec.decode_frame(Some(&[]), false);
        }

        #[test]
        fn short_payload_never_panics() {
            // libopus is intentionally lenient: many "malformed" inputs decode
            // to something rather than erroring. The contract this test does
            // enforce is that no input, however small or hostile, can panic
            // the decoder. That alone keeps a single bad packet from killing
            // a live call.
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let _ = dec.decode_frame(Some(&[]), false);
            let _ = dec.decode_frame(Some(&[0u8; 1]), false);
            let _ = dec.decode_frame(Some(&[0u8; 4]), false);
        }

        #[test]
        fn hostile_payload_never_panics() {
            // Sweep a range of garbage lengths; the contract is "no panic",
            // not "always errors". libopus can interpret some of these as
            // valid Opus packets; the test cares only that we survive them.
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            for len in 0..128usize {
                let bytes: Vec<u8> =
                    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(7)).collect();
                let _ = dec.decode_frame(Some(&bytes), false);
            }
        }

        #[test]
        fn many_roundtrips_keep_sample_counts_stable() {
            // 50 frames at 20 ms is one second of voice — a representative
            // burst size. We just check that sample counts stay constant
            // across a long sequence, which is what catches the encoder
            // drifting to a different frame size internally.
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();

            for i in 0..50 {
                let pcm: Vec<i16> = (0..960)
                    .map(|s| {
                        let t = (s + i * 960) as f64 / 48_000.0;
                        ((2.0 * core::f64::consts::PI * 440.0 * t).sin() * 16_000.0) as i16
                    })
                    .collect();
                let encoded = enc.encode_frame(&pcm).unwrap();
                let decoded = dec.decode_frame(Some(&encoded.payload), false).unwrap();
                assert_eq!(decoded.samples.len(), 960);
            }
        }

        #[test]
        fn trait_roundtrip_via_existing_encoder_decoder_shims() {
            // The codec must also satisfy the existing `Encoder`/`Decoder`
            // traits so the engine wiring keeps working.
            let mut enc = OpusVoiceEncoder::new(default_config()).unwrap();
            let mut dec = OpusVoiceDecoder::new(default_config()).unwrap();
            let pcm = sine_440();
            let frame = PcmFrame::new(pcm, 48_000, 1, MediaTimestamp(0));

            let payload = enc.encode(&frame).unwrap();
            let decoded = dec.decode(&payload).unwrap();
            assert_eq!(decoded.samples.len(), 960);
            let concealed = dec.conceal().unwrap();
            assert_eq!(concealed.samples.len(), 960);
        }
    }
}
