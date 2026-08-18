//! The advertisement payload (§62–§64).
//!
//! This is the only thing Anvil broadcasts to anyone who happens to be
//! listening, so it is worth being deliberate about what goes in it.
//!
//! The hard constraint is size: Wi-Fi Aware service-specific info is capped at
//! roughly [`crate::transport::aware::MAX_ADVERTISEMENT_BYTES`], and mDNS TXT
//! records want to stay small too. A full 32-byte identity key plus a room id
//! plus a display name does not comfortably fit, and padding it out would slow
//! discovery on the transport where discovery is already slowest.
//!
//! So the advertisement carries an 8-byte **fingerprint** of the identity key,
//! not the key. That is enough to correlate the same device seen over two
//! transports (§65) and to recognise a peer met before. It is *not* enough to
//! authenticate anything, and nothing here should be treated as true:
//!
//! * a fingerprint is trivially copied by anyone nearby;
//! * a display name is whatever a stranger typed;
//! * a room hint proves nothing about the room.
//!
//! Identity becomes real at the handshake, when the peer proves possession of
//! the private key behind the full identity (§45, §67). Until then this data is
//! a routing hint and a UI convenience, and the UI must present it as
//! unconfirmed. Everything in this module is untrusted input.

use crate::{ProtocolError, Result, RoomId, PROTOCOL_VERSION};

/// Length of the truncated identity fingerprint carried in advertisements.
///
/// 8 bytes gives a ~1 in 2^32 chance of accidental collision among the handful
/// of devices in radio range — fine for correlation, useless for security,
/// which is the correct division of labour.
pub const FINGERPRINT_LEN: usize = 8;

const FLAG_HOSTING: u8 = 0b0000_0001;
const MAX_NAME_LEN: usize = 48;

/// Truncated identity fingerprint.
pub type Fingerprint = [u8; FINGERPRINT_LEN];

/// What a device puts on the air.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Advertisement {
    /// Wire version, so a peer running an incompatible build is visible as
    /// incompatible rather than invisible.
    pub version: u8,
    /// Truncated identity fingerprint. Unverified.
    pub fingerprint: Fingerprint,
    /// Truncated room id, if this device is hosting a joinable room.
    pub room_hint: Option<u32>,
    /// Display name. Unverified, attacker-controlled, and possibly rude.
    pub display_name: String,
}

impl Advertisement {
    /// Build an advertisement for this node.
    #[must_use]
    pub fn new(fingerprint: Fingerprint, hosting: Option<RoomId>, display_name: &str) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            fingerprint,
            room_hint: hosting.map(|r| r.route_id()),
            display_name: truncate_chars(display_name, MAX_NAME_LEN),
        }
    }

    /// Serialise for a TXT record or Aware service info.
    ///
    /// ```text
    /// 0      version
    /// 1      flags
    /// 2..10  fingerprint
    /// [10..14] room hint, present only if the hosting flag is set
    /// next   name length
    /// then   name bytes (UTF-8)
    /// ```
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let name = self.display_name.as_bytes();
        let mut out = Vec::with_capacity(11 + 4 + name.len());

        out.push(self.version);
        out.push(if self.room_hint.is_some() { FLAG_HOSTING } else { 0 });
        out.extend_from_slice(&self.fingerprint);
        if let Some(hint) = self.room_hint {
            out.extend_from_slice(&hint.to_be_bytes());
        }
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out
    }

    /// Parse an advertisement seen on the air.
    ///
    /// Every failure path here is reachable by a hostile neighbour sending
    /// deliberate garbage, so this returns errors rather than panicking, and
    /// slices only after checking lengths.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let need = 2 + FINGERPRINT_LEN + 1;
        if bytes.len() < need {
            return Err(ProtocolError::Truncated { got: bytes.len(), need }.into());
        }

        let version = bytes[0];
        if version != PROTOCOL_VERSION {
            return Err(
                ProtocolError::VersionMismatch { theirs: version, ours: PROTOCOL_VERSION }.into()
            );
        }

        let flags = bytes[1];
        let mut fingerprint = [0u8; FINGERPRINT_LEN];
        fingerprint.copy_from_slice(&bytes[2..2 + FINGERPRINT_LEN]);
        let mut cursor = 2 + FINGERPRINT_LEN;

        let room_hint = if flags & FLAG_HOSTING != 0 {
            if bytes.len() < cursor + 4 {
                return Err(ProtocolError::Truncated { got: bytes.len(), need: cursor + 4 }.into());
            }
            let hint = u32::from_be_bytes([
                bytes[cursor],
                bytes[cursor + 1],
                bytes[cursor + 2],
                bytes[cursor + 3],
            ]);
            cursor += 4;
            Some(hint)
        } else {
            None
        };

        if bytes.len() <= cursor {
            return Err(ProtocolError::Truncated { got: bytes.len(), need: cursor + 1 }.into());
        }
        let name_len = bytes[cursor] as usize;
        cursor += 1;

        if name_len > MAX_NAME_LEN {
            return Err(ProtocolError::Malformed("display name too long").into());
        }
        if bytes.len() < cursor + name_len {
            return Err(
                ProtocolError::Truncated { got: bytes.len(), need: cursor + name_len }.into()
            );
        }

        // Lossy on purpose. A neighbour sending invalid UTF-8 should produce a
        // slightly mangled name in the peer list, not a discovery failure.
        let display_name = String::from_utf8_lossy(&bytes[cursor..cursor + name_len]).into_owned();

        Ok(Self { version, fingerprint, room_hint, display_name })
    }

    /// Whether this device is advertising a joinable room.
    #[must_use]
    pub const fn is_hosting(&self) -> bool {
        self.room_hint.is_some()
    }
}

fn truncate_chars(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_owned();
    }
    // Cut on a character boundary, never mid-codepoint.
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FP: Fingerprint = [1, 2, 3, 4, 5, 6, 7, 8];

    #[test]
    fn round_trips_without_a_room() {
        let ad = Advertisement::new(FP, None, "Adeleke");
        let decoded = Advertisement::decode(&ad.encode()).unwrap();
        assert_eq!(decoded, ad);
        assert!(!decoded.is_hosting());
    }

    #[test]
    fn round_trips_with_a_room() {
        let room = RoomId::generate();
        let ad = Advertisement::new(FP, Some(room), "Adeleke");
        let decoded = Advertisement::decode(&ad.encode()).unwrap();

        assert_eq!(decoded, ad);
        assert_eq!(decoded.room_hint, Some(room.route_id()));
    }

    #[test]
    fn fits_within_the_wifi_aware_budget() {
        let ad = Advertisement::new(FP, Some(RoomId::generate()), &"n".repeat(100));
        assert!(
            ad.encode().len() <= crate::transport::aware::MAX_ADVERTISEMENT_BYTES,
            "advertisement was {} bytes",
            ad.encode().len()
        );
    }

    #[test]
    fn rejects_a_future_version_instead_of_guessing() {
        let mut bytes = Advertisement::new(FP, None, "x").encode();
        bytes[0] = 99;
        assert!(matches!(
            Advertisement::decode(&bytes),
            Err(crate::Error::Protocol(ProtocolError::VersionMismatch { theirs: 99, .. }))
        ));
    }

    #[test]
    fn survives_hostile_truncation_at_every_length() {
        let full = Advertisement::new(FP, Some(RoomId::generate()), "Adeleke").encode();
        for cut in 0..full.len() {
            // Must return an error, never panic.
            let _ = Advertisement::decode(&full[..cut]);
        }
    }

    #[test]
    fn survives_a_lying_length_prefix() {
        let mut bytes = Advertisement::new(FP, None, "hi").encode();
        let last = bytes.len() - 3;
        bytes[last] = 200; // claims a 200-byte name that is not there
        assert!(Advertisement::decode(&bytes).is_err());
    }

    #[test]
    fn invalid_utf8_names_degrade_rather_than_fail() {
        let mut bytes = Advertisement::new(FP, None, "ab").encode();
        let len = bytes.len();
        bytes[len - 1] = 0xff;
        let decoded = Advertisement::decode(&bytes).expect("should not reject the whole peer");
        assert_eq!(decoded.fingerprint, FP);
    }

    #[test]
    fn long_names_are_cut_on_character_boundaries() {
        let ad = Advertisement::new(FP, None, &"é".repeat(60));
        assert!(ad.display_name.len() <= MAX_NAME_LEN);
        // Would have panicked on construction if it cut mid-codepoint.
        assert!(Advertisement::decode(&ad.encode()).is_ok());
    }
}
