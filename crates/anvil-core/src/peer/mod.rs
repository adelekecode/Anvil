//! One-to-one peer relationships.
//!
//! Between two Anvil devices there is exactly one authenticated session, and
//! everything rides it:
//!
//! ```text
//!   Peer session  (one handshake, one key agreement, one failover story)
//!   ├── voice     when there is a call
//!   ├── chat      whenever either side types
//!   └── control   always
//! ```
//!
//! Calling somebody and messaging them are therefore not separate features with
//! separate connections — they are two uses of the same relationship. That is
//! why adding chat cost no new transport, no new trust model and no new
//! cryptography.
//!
//! Note what is *not* here: the handshake itself lives in
//! [`crate::crypto::handshake`], because it is cryptography, and trust lives in
//! [`crate::identity::KnownPeers`], because it is identity. This module is about
//! what two peers are currently doing with each other.

mod call;

pub use call::{CallEnded, CallState, InvalidTransition, RING_TIMEOUT};

use crate::identity::TrustState;
use crate::transport::PathKind;
use crate::PeerId;

/// A peer as the home screen shows them.
///
/// Assembled from discovery, transport and the known-peer store, so the UI has
/// one thing to render rather than joining three sources itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSummary {
    /// Cryptographic identity, once confirmed.
    pub peer_id: Option<PeerId>,
    /// Provisional correlation key from discovery.
    pub fingerprint: [u8; crate::discovery::FINGERPRINT_LEN],
    /// Advertised or remembered name.
    pub display_name: String,
    /// Round-trip time on the active path, in milliseconds, once measured.
    ///
    /// `None` means "not measured yet", which the UI must render differently
    /// from a measured zero — an unprobed path is not a fast one.
    pub rtt_ms: Option<u32>,
    /// Transport currently carrying traffic, if connected.
    pub transport: Option<PathKind>,
    /// Whether this device has met them before.
    pub known: bool,
    /// Trust state, for peers we know.
    pub trust: Option<TrustState>,
    /// Whether they are advertising a joinable room.
    pub hosting_room: bool,
}

impl PeerSummary {
    /// Whether the UI should show a trust warning next to this peer.
    #[must_use]
    pub fn needs_warning(&self) -> bool {
        self.trust.is_some_and(TrustState::needs_warning)
    }

    /// Whether identity has been cryptographically confirmed.
    ///
    /// Until this is true, the display name is an unverified claim by whoever
    /// is broadcasting — which the UI must not present as a person.
    #[must_use]
    pub const fn is_confirmed(&self) -> bool {
        self.peer_id.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> PeerSummary {
        PeerSummary {
            peer_id: None,
            fingerprint: [1; 8],
            display_name: "Daniel".into(),
            rtt_ms: None,
            transport: None,
            known: false,
            trust: None,
            hosting_room: false,
        }
    }

    #[test]
    fn an_unconfirmed_peer_is_not_presented_as_identified() {
        let peer = summary();
        assert!(!peer.is_confirmed());
        assert!(!peer.needs_warning());
    }

    #[test]
    fn a_changed_identity_raises_a_warning() {
        let peer = PeerSummary {
            peer_id: Some(PeerId([2; 32])),
            trust: Some(TrustState::Changed),
            known: true,
            ..summary()
        };

        assert!(peer.is_confirmed());
        assert!(peer.needs_warning());
    }

    #[test]
    fn verified_and_unverified_peers_do_not_warn() {
        for trust in [TrustState::Verified, TrustState::Unverified] {
            let peer = PeerSummary { trust: Some(trust), known: true, ..summary() };
            assert!(!peer.needs_warning());
        }
    }

    #[test]
    fn unmeasured_latency_is_distinct_from_zero() {
        // A path that has not been probed must not render as instant.
        let unmeasured = summary();
        let measured = PeerSummary { rtt_ms: Some(0), ..summary() };

        assert_eq!(unmeasured.rtt_ms, None);
        assert_ne!(unmeasured.rtt_ms, measured.rtt_ms);
    }
}
