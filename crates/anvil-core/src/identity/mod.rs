//! Decentralised identity.
//!
//! Anvil has no accounts. There is no signup, no login, no password, no phone
//! number, no OAuth, no session server and no user database — not as an
//! omission, but as the architecture.
//!
//! ```text
//!   Install
//!      ↓
//!   "Display name: [ Femi ]"
//!      ↓
//!   generate keypair on device        ← the only thing that happens
//!      ↓
//!   store it in platform secure storage
//!      ↓
//!   ready
//! ```
//!
//! Every subsequent launch loads the local profile and starts discovery. There
//! is no screen between opening the app and using it.
//!
//! ## The two names for a person
//!
//! | | Display name | PeerId |
//! |---|---|---|
//! | For | humans | the protocol |
//! | Unique? | **no** | yes |
//! | Chosen by | the user | the key |
//! | Trustworthy? | only once verified | inherently |
//!
//! Two people can both be "Femi". Anvil knows they are `anv_a82…` and
//! `anv_c93…`, and the UI can disambiguate with a [`Fingerprint`]. Conflating
//! the two — treating a name as an identity anywhere in the protocol — is the
//! mistake this split exists to prevent.
//!
//! ## Trust
//!
//! Trust-on-first-use, plus optional out-of-band verification. See
//! [`known_peer`] for what that does and does not defend against; the short
//! version is that it catches someone claiming a name you already trust, and
//! cannot help you the very first time you meet somebody.

mod fingerprint;
mod known_peer;
mod profile;

pub use fingerprint::{Fingerprint, VerificationPayload, LONG_BYTES, SHORT_BYTES};
pub use known_peer::{KnownPeer, KnownPeers, TofuOutcome, TrustState};
pub use profile::{validate_name, LocalProfile, NameError, MAX_DISPLAY_NAME};

// Re-exported so identity code has one obvious import path, rather than
// reaching across into `crypto` for the type it is built around.
pub use crate::crypto::{DeviceIdentity, PublicIdentity};

use crate::PeerId;

/// Prefix on displayed peer identifiers.
///
/// Exists so that a string appearing in a log, a bug report or a support
/// conversation is recognisable as an Anvil identity rather than an anonymous
/// hex blob.
pub const PEER_ID_PREFIX: &str = "anv_";

/// Full displayable identifier: `anv_` followed by 64 hex characters.
///
/// Used where the exact identity matters — verification screens, diagnostics,
/// export. Never in a peer list, where it is unreadable.
#[must_use]
pub fn peer_id_string(peer: PeerId) -> String {
    use core::fmt::Write as _;

    let mut out = String::with_capacity(PEER_ID_PREFIX.len() + 64);
    out.push_str(PEER_ID_PREFIX);
    for byte in peer.0 {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// Abbreviated identifier: `anv_7ab93…`.
///
/// For places that need to show *an* identity without claiming to show all of
/// it. The ellipsis is deliberate — it signals truncation, so nobody compares
/// two of these and believes they have compared identities.
#[must_use]
pub fn peer_id_short_string(peer: PeerId) -> String {
    format!("{PEER_ID_PREFIX}{}…", peer.short()[..5].to_owned())
}

/// Parse a displayed identifier back to a [`PeerId`].
///
/// Accepts the full form only. The abbreviated form is lossy by design and must
/// never round-trip, or it would become a de facto identifier and people would
/// start comparing them.
#[must_use]
pub fn parse_peer_id(text: &str) -> Option<PeerId> {
    let hex = text.strip_prefix(PEER_ID_PREFIX)?;
    if hex.len() != 64 {
        return None;
    }

    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
    }
    Some(PeerId(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_identifiers_round_trip() {
        let peer = PeerId([0x7A; 32]);
        let text = peer_id_string(peer);

        assert!(text.starts_with("anv_"));
        assert_eq!(text.len(), 4 + 64);
        assert_eq!(parse_peer_id(&text), Some(peer));
    }

    #[test]
    fn short_identifiers_are_marked_as_truncated_and_do_not_parse() {
        let peer = PeerId([0x7A, 0xB9, 0x30, 0x11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                           0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let short = peer_id_short_string(peer);

        assert!(short.starts_with("anv_7ab93"));
        assert!(short.ends_with('…'), "truncation must be visible");
        assert_eq!(parse_peer_id(&short), None, "a truncated id must never round-trip");
    }

    #[test]
    fn malformed_identifiers_are_rejected() {
        for text in ["", "anv_", "7a7a7a", &format!("anv_{}", "zz".repeat(32)), "anv_7a"] {
            assert!(parse_peer_id(text).is_none(), "accepted {text:?}");
        }
    }
}
