//! Human-checkable fingerprints.
//!
//! A fingerprint exists for one purpose: so two people standing next to each
//! other can compare something short enough to read aloud and confirm they are
//! talking to who they think.
//!
//! ```text
//!   Femi
//!   7A:42:19:BC
//! ```
//!
//! ## Why four bytes
//!
//! It is a trade-off between what a person will actually check and what an
//! attacker must do to forge.
//!
//! Four bytes is 32 bits. Producing a *second* identity whose fingerprint
//! matches a given one costs about 2³² key generations — hours of compute,
//! entirely achievable. So a 4-byte fingerprint is **not** sufficient on its own
//! against a determined targeted attacker.
//!
//! It is, however, sufficient for what it is used for here: confirming that the
//! Daniel in front of you is the Daniel your phone remembers, against an
//! attacker who is nearby, opportunistic, and did not know in advance whose
//! identity they wanted to collide with. And a fingerprint nobody reads because
//! it is 64 characters long protects nothing at all.
//!
//! Two things follow:
//!
//! * QR verification ([`crate::identity::VerificationPayload`]) carries the
//!   **full** public key, not the fingerprint. Anything security-critical uses
//!   the full key.
//! * [`Fingerprint::long`] exists for a "show full fingerprint" affordance, for
//!   people who want to compare more.

use core::fmt;

use crate::PeerId;

/// Bytes shown in the short form.
pub const SHORT_BYTES: usize = 4;

/// Bytes shown in the long form.
pub const LONG_BYTES: usize = 16;

/// A displayable fingerprint of a peer identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint {
    bytes: [u8; LONG_BYTES],
}

impl Fingerprint {
    /// Fingerprint of a peer.
    #[must_use]
    pub fn of(peer: PeerId) -> Self {
        let mut bytes = [0u8; LONG_BYTES];
        bytes.copy_from_slice(&peer.0[..LONG_BYTES]);
        Self { bytes }
    }

    /// Short form for the peer list: `7A:42:19:BC`.
    #[must_use]
    pub fn short(&self) -> String {
        group(&self.bytes[..SHORT_BYTES], 1)
    }

    /// Long form for a verification screen, in space-separated pairs:
    /// `7A42 19BC 3F08 …`.
    ///
    /// Grouped rather than run together because people compare grouped digits
    /// far more reliably — the same reason IBANs and card numbers are chunked.
    #[must_use]
    pub fn long(&self) -> String {
        group(&self.bytes, 2)
    }

    /// Raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; LONG_BYTES] {
        &self.bytes
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({})", self.short())
    }
}

/// Uppercase hex, `bytes_per_group` bytes per group.
///
/// Separator is `:` for single bytes (the familiar fingerprint look) and a
/// space for larger groups.
fn group(bytes: &[u8], bytes_per_group: usize) -> String {
    use fmt::Write as _;

    let separator = if bytes_per_group == 1 { ':' } else { ' ' };
    let mut out = String::new();

    for (index, chunk) in bytes.chunks(bytes_per_group).enumerate() {
        if index > 0 {
            out.push(separator);
        }
        for byte in chunk {
            let _ = write!(out, "{byte:02X}");
        }
    }
    out
}

/// What a QR code carries for out-of-band verification.
///
/// Carries the **full** public key, not a fingerprint. The fingerprint is for
/// eyes; this is for machines, and there is no reason to weaken it.
///
/// Scanning this proves nothing on its own — an attacker could show you their
/// own QR code. What it proves is that *the key in this QR belongs to the person
/// physically holding this screen*, which is exactly the trust anchor Anvil has
/// and a certificate authority does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerificationPayload {
    /// Wire version, so a future format is recognisable rather than mysterious.
    pub version: u8,
    /// Full public identity key.
    pub public_key: [u8; 32],
    /// Display name at the time of verification, so the UI can say "you
    /// verified this key as Daniel".
    pub display_name: String,
}

impl VerificationPayload {
    /// Encode for a QR code: `anvil:v1:<hex key>:<name>`.
    ///
    /// A URI-ish text form rather than binary so it survives being pasted into
    /// a message, read aloud in an emergency, or debugged by eye.
    #[must_use]
    pub fn encode(&self) -> String {
        use fmt::Write as _;

        let mut key = String::with_capacity(64);
        for byte in self.public_key {
            let _ = write!(key, "{byte:02x}");
        }
        format!("anvil:v{}:{}:{}", self.version, key, self.display_name)
    }

    /// Parse a scanned code. Returns `None` for anything unrecognised — a
    /// wrong QR code must fail visibly, never partially succeed.
    #[must_use]
    pub fn decode(text: &str) -> Option<Self> {
        let rest = text.strip_prefix("anvil:v")?;
        let (version, rest) = rest.split_once(':')?;
        let version: u8 = version.parse().ok()?;
        let (key_hex, display_name) = rest.split_once(':')?;

        if key_hex.len() != 64 {
            return None;
        }

        let mut public_key = [0u8; 32];
        for (index, byte) in public_key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(key_hex.get(index * 2..index * 2 + 2)?, 16).ok()?;
        }

        Some(Self { version, public_key, display_name: display_name.to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer_with(prefix: [u8; 4]) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[..4].copy_from_slice(&prefix);
        PeerId(bytes)
    }

    #[test]
    fn short_form_is_readable_aloud() {
        let fingerprint = Fingerprint::of(peer_with([0x7A, 0x42, 0x19, 0xBC]));
        assert_eq!(fingerprint.short(), "7A:42:19:BC");
        assert_eq!(fingerprint.to_string(), "7A:42:19:BC");
    }

    #[test]
    fn long_form_is_grouped_for_comparison() {
        let fingerprint = Fingerprint::of(PeerId([0xAB; 32]));
        let long = fingerprint.long();

        assert_eq!(long.split(' ').count(), 8);
        assert!(long.starts_with("ABAB ABAB"));
    }

    #[test]
    fn different_identities_have_different_fingerprints() {
        assert_ne!(
            Fingerprint::of(peer_with([1, 2, 3, 4])),
            Fingerprint::of(peer_with([1, 2, 3, 5]))
        );
    }

    #[test]
    fn verification_payload_round_trips() {
        let payload = VerificationPayload {
            version: 1,
            public_key: [0x5C; 32],
            display_name: "Daniel".into(),
        };

        let decoded = VerificationPayload::decode(&payload.encode()).unwrap();
        assert_eq!(decoded, payload);
    }

    #[test]
    fn verification_carries_the_full_key_not_the_fingerprint() {
        let payload =
            VerificationPayload { version: 1, public_key: [0x11; 32], display_name: "D".into() };

        // 64 hex characters = the whole key.
        assert!(payload.encode().contains(&"11".repeat(32)));
    }

    #[test]
    fn names_containing_colons_survive_the_round_trip() {
        let payload = VerificationPayload {
            version: 1,
            public_key: [0x22; 32],
            display_name: "Daniel: the second".into(),
        };

        assert_eq!(
            VerificationPayload::decode(&payload.encode()).unwrap().display_name,
            "Daniel: the second"
        );
    }

    #[test]
    fn a_wrong_qr_code_fails_visibly() {
        for text in [
            "",
            "hello",
            "anvil:v1",
            "anvil:v1:short:Daniel",
            "anvil:vX:1111:Daniel",
            "https://example.com",
            &format!("anvil:v1:{}:Daniel", "zz".repeat(32)),
        ] {
            assert!(VerificationPayload::decode(text).is_none(), "accepted {text:?}");
        }
    }
}
