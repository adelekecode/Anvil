//! The audio pipeline (§26).
//!
//! ```text
//!   send:  mic ─► PCM ─► VAD ─► Opus ─► encrypt ─► packet ─► datagram
//!   recv:  datagram ─► parse ─► authenticate+decrypt ─► jitter ─► Opus ─► mix ─► speaker
//! ```
//!
//! Two properties of this ordering are load-bearing:
//!
//! * **VAD runs before the encoder.** Not encoding silence saves CPU as well as
//!   bandwidth, and on a phone in a four-person room that is real battery.
//! * **Authentication runs before the decoder** (§80). The Opus decoder is a
//!   large C surface; it must only ever see bytes that a room member authored.
//!   Decoding first and checking later would hand an attacker within radio
//!   range a direct path into it.
//!
//! Everything here is platform-independent. The only things the OS provides are
//! raw capture frames in and mixed frames out, through
//! [`crate::platform::AudioAdapter`], which is what lets the jitter buffer,
//! VAD and mixer be tested exhaustively without a device — see the tests in
//! each submodule.

pub mod codec;
#[cfg(feature = "desktop")]
pub mod desktop;
pub mod frame;
pub mod jitter;
pub mod mixer;
pub mod opus;
#[cfg(feature = "opus")]
pub mod pipeline;
pub mod resampler;
pub mod ring_buffer;
pub mod vad;

pub use codec::{Decoder, Encoder, NullDecoder, NullEncoder};
pub use frame::{samples_per_frame, EncodedFrame, PcmFrame};
pub use jitter::{JitterBuffer, Playout};
pub use mixer::Mixer;
pub use vad::VoiceActivityDetector;

#[cfg(feature = "opus")]
pub use opus::{
    DecodedAudioFrame, EncodedAudioFrame, OpusConfig, OpusVoiceDecoder, OpusVoiceEncoder,
};
pub use resampler::{AudioResampler, TARGET_FRAME_SAMPLES, TARGET_SAMPLE_RATE};
pub use ring_buffer::PcmRingBuffer;

#[cfg(feature = "opus")]
pub use pipeline::{CapturePipeline, PlaybackPipeline};

#[cfg(feature = "desktop")]
pub use desktop::CpalLoop;
