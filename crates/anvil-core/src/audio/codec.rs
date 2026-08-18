//! Codec interface (§27).
//!
//! Anvil uses Opus and will not write a codec (§109). This module exists so the
//! rest of the audio pipeline depends on a trait rather than on a specific
//! encoder — which keeps the pipeline testable without libopus, and makes the
//! `opus` cargo feature genuinely optional rather than load-bearing.

use crate::audio::frame::PcmFrame;
use crate::Result;

/// Encodes PCM to a compressed payload.
pub trait Encoder: Send + core::fmt::Debug {
    /// Encode one frame.
    fn encode(&mut self, frame: &PcmFrame) -> Result<Vec<u8>>;

    /// Change the target bitrate at runtime.
    ///
    /// Phase 7 wants this: when path metrics show congestion, dropping bitrate
    /// is a far better response than dropping frames.
    fn set_bitrate(&mut self, bps: u32) -> Result<()>;

    /// Largest payload the encoder may produce, so frames always fit one
    /// datagram on the current path.
    fn set_max_payload(&mut self, bytes: usize) -> Result<()>;
}

/// Decodes a compressed payload to PCM.
pub trait Decoder: Send + core::fmt::Debug {
    /// Decode one frame.
    fn decode(&mut self, payload: &[u8]) -> Result<PcmFrame>;

    /// Produce a concealment frame for a packet that never arrived (§30).
    ///
    /// Opus synthesises this from its internal state — it is not silence, and
    /// substituting silence instead is immediately audible as a click.
    fn conceal(&mut self) -> Result<PcmFrame>;
}

/// An encoder that produces nothing. Lets the pipeline be exercised end to end
/// without libopus present.
#[derive(Debug, Default)]
pub struct NullEncoder;

impl Encoder for NullEncoder {
    fn encode(&mut self, _frame: &PcmFrame) -> Result<Vec<u8>> {
        Err(crate::Error::NotImplemented("audio::codec: no encoder (build with --features opus)"))
    }
    fn set_bitrate(&mut self, _bps: u32) -> Result<()> {
        Ok(())
    }
    fn set_max_payload(&mut self, _bytes: usize) -> Result<()> {
        Ok(())
    }
}

/// A decoder that produces nothing.
#[derive(Debug, Default)]
pub struct NullDecoder;

impl Decoder for NullDecoder {
    fn decode(&mut self, _payload: &[u8]) -> Result<PcmFrame> {
        Err(crate::Error::NotImplemented("audio::codec: no decoder (build with --features opus)"))
    }
    fn conceal(&mut self) -> Result<PcmFrame> {
        Err(crate::Error::NotImplemented("audio::codec: no decoder (build with --features opus)"))
    }
}
