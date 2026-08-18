//! QUIC as the local secure transport (§24, §25).
//!
//! QUIC is used so Anvil does not have to invent connection state, congestion
//! control, or link security. It gives two delivery modes over one connection,
//! which maps exactly onto the two kinds of traffic Anvil has:
//!
//! | Traffic | QUIC mechanism | Why |
//! |---|---|---|
//! | Join, membership, key distribution, election, relay switch | reliable stream | must arrive, must be ordered |
//! | Voice frames, probes, heartbeats | unreliable datagram | a late frame is worse than a missing one |
//!
//! Putting media on a reliable stream would be a serious mistake: one lost
//! packet would stall every frame behind it, converting a 20 ms gap the Opus
//! decoder can conceal into a multi-hundred-millisecond stall the user hears as
//! the call breaking.
//!
//! ## Authentication
//!
//! Anvil has no CA, no server names and no Internet, so QUIC's TLS layer is used
//! with **raw public keys tied to the device Ed25519 identity**, not X.509 name
//! validation. A peer is trusted because its [`crate::PeerId`] matches the
//! identity presented, verified against what discovery advertised — not because
//! a certificate chains anywhere.
//!
//! Note what this layer is *not*. QUIC secures one hop. Media is separately
//! end-to-end encrypted (§42–§52), because a relay terminates QUIC and would
//! otherwise see plaintext. Transport security and media security are different
//! layers protecting against different adversaries, and neither substitutes for
//! the other.
//!
//! ## Relay hops
//!
//! A relayed packet crosses two QUIC connections: sender→relay and relay→
//! receiver. Each is separately encrypted at the transport layer and separately
//! terminated. The media envelope inside is untouched by both.
//!
//! ## Phase
//!
//! Phase 1. Enabled by the `quic` feature; the type below is the seam the
//! implementation slots into.

use crate::transport::PathKind;

/// Datagram sizes negotiated per path; these are the floors used before a peer
/// reports its own limit.
#[must_use]
pub const fn conservative_datagram_size(kind: PathKind) -> usize {
    match kind {
        PathKind::Lan => super::lan::CONSERVATIVE_DATAGRAM_SIZE,
        PathKind::WifiAware => super::aware::CONSERVATIVE_DATAGRAM_SIZE,
    }
}

/// ALPN identifier for Anvil over QUIC.
///
/// Versioned, so a future incompatible wire format simply fails to negotiate
/// rather than connecting and then misbehaving.
pub const ALPN: &[u8] = b"anvil/1";

/// Idle timeout before QUIC itself declares the connection dead.
///
/// Longer than [`crate::TransportConfig::path_timeout`] on purpose: Anvil's own
/// staleness detection should notice and fail over first, because it can move
/// media to a standby path, whereas QUIC can only give up.
pub const IDLE_TIMEOUT_SECS: u64 = 10;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aware_datagrams_are_sized_no_larger_than_lan() {
        assert!(
            conservative_datagram_size(PathKind::WifiAware)
                <= conservative_datagram_size(PathKind::Lan)
        );
    }
}
