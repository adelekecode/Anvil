//! Join codes and decentralised room discovery.
//!
//! A room has two identifiers doing two different jobs:
//!
//! | | `RoomId` | `JoinCode` |
//! |---|---|---|
//! | Looks like | 128 random bits | `ANV-7FK2-P9W4` |
//! | For | the protocol | a human to read aloud |
//! | Learned | at `RoomAccept` | from the host, out of band |
//!
//! ## How discovery works without a registry
//!
//! Nothing anywhere records that a room exists. The participants *are* the room.
//! So joining works by derivation rather than lookup:
//!
//! ```text
//!   host                              joiner
//!   ────                              ──────
//!   JoinCode  ─┐                  ┌─  user types the same JoinCode
//!              ▼                  ▼
//!        discovery token   ==   discovery token
//!              │                  │
//!   advertises it locally  ◄──────┘ "who is advertising this token?"
//!              │
//!              └──►  a member answers, membership handshake follows
//! ```
//!
//! The code is never sent to a server, because there is no server. It is not
//! even sent over the network in the clear during discovery — what travels is
//! the derived token.
//!
//! ## What a join code is and is not
//!
//! **It is a bootstrap convenience. It is not access control.**
//!
//! Eight base32 characters is 40 bits. An attacker who observes the advertised
//! token can brute-force the code offline — 2⁴⁰ is hours of ordinary compute,
//! not a research project. Lengthening the code would help and would also make
//! it unreadable over the phone, which is the entire point of having one.
//!
//! So the honest position, which the UI should reflect:
//!
//! * a join code stops *casual* joining by someone who did not hear it;
//! * a room that needs real access control uses
//!   [`crate::room::AdmissionPolicy::HostApproval`], where a human decides;
//! * cryptographic membership is enforced separately and always — passing
//!   admission still requires completing an authenticated handshake, and the
//!   key epoch advances so nobody who was not admitted holds usable keys.
//!
//! Guessing a code gets an attacker as far as *asking to join*. Nothing more.

use core::fmt;

use crate::RoomId;

#[cfg(feature = "crypto")]
use sha2::Sha256;

/// Crockford base32: no `I`, `L`, `O` or `U`.
///
/// `I`/`L` are excluded because they are indistinguishable from `1` when read
/// aloud or in most fonts; `O` from `0`; `U` because excluding it avoids
/// accidental profanity. A code that gets mistyped every third time is worse
/// than a slightly shorter alphabet.
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Characters in the code, excluding the prefix and hyphens.
const CODE_LEN: usize = 8;

/// Human-facing room code, e.g. `ANV-7FK2-P9W4`.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct JoinCode {
    /// The 8 code characters, without prefix or separators.
    chars: [u8; CODE_LEN],
}

impl JoinCode {
    /// Generate a fresh random code.
    #[must_use]
    pub fn generate() -> Self {
        let mut raw = [0u8; CODE_LEN];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut raw);

        let mut chars = [0u8; CODE_LEN];
        for (out, byte) in chars.iter_mut().zip(raw.iter()) {
            *out = ALPHABET[(*byte as usize) % ALPHABET.len()];
        }
        Self { chars }
    }

    /// Parse what a user typed, generously.
    ///
    /// Accepts lower case, missing or extra hyphens, spaces, and a missing
    /// `ANV-` prefix. Also corrects the ambiguous characters the alphabet
    /// excludes — someone reading `0` aloud as "oh" and the listener typing `O`
    /// should simply work, rather than producing "no such room", which is an
    /// unhelpful thing to tell someone who typed what they heard.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let normalised: Vec<u8> = input
            .trim()
            .to_ascii_uppercase()
            .bytes()
            .filter(|b| b.is_ascii_alphanumeric())
            .map(|b| match b {
                b'I' | b'L' => b'1',
                b'O' => b'0',
                b'U' => b'V',
                other => other,
            })
            .collect();

        // Drop an optional leading ANV prefix.
        let body: &[u8] = match normalised.strip_prefix(b"ANV") {
            Some(rest) => rest,
            None => &normalised,
        };

        if body.len() != CODE_LEN {
            return None;
        }
        if !body.iter().all(|b| ALPHABET.contains(b)) {
            return None;
        }

        let mut chars = [0u8; CODE_LEN];
        chars.copy_from_slice(body);
        Some(Self { chars })
    }

    /// The code as typed and displayed: `ANV-7FK2-P9W4`.
    #[must_use]
    pub fn formatted(&self) -> String {
        let text = core::str::from_utf8(&self.chars).unwrap_or("????????");
        format!("ANV-{}-{}", &text[..4], &text[4..])
    }

    /// The bare 8 characters, without prefix or hyphens.
    #[must_use]
    pub fn raw(&self) -> String {
        core::str::from_utf8(&self.chars).unwrap_or_default().to_owned()
    }

    /// The token advertised locally so joiners can find this room.
    ///
    /// Derived from the code so that both sides compute the same value with no
    /// coordination. It occupies the same 4-byte advertisement slot as the room
    /// hint, which is all the budget Wi-Fi Aware allows.
    ///
    /// On the `crypto` feature this is HKDF-SHA-256, domain-separated, and
    /// truncated to 32 bits. The FNV-1a fallback exists only so the no-crypto
    /// build — which is the one that ships in the Phase 0 scaffold — still
    /// has a deterministic, well-distributed token to advertise. The token is
    /// not a security boundary either way: a 40-bit join code is the actual
    /// admission check, and a 32-bit truncation of any hash is brute-forceable
    /// in microseconds. The point of using HKDF here is to keep the *shape*
    /// honest so the future wire format does not have to change.
    #[must_use]
    pub fn discovery_token(&self) -> u32 {
        #[cfg(feature = "crypto")]
        {
            use hkdf::Hkdf;
            let hk = Hkdf::<Sha256>::new(None, &self.chars);
            let info = b"anvil-room-token/v1";
            let mut okm = [0u8; 4];
            hk.expand(info, &mut okm)
                .expect("4-byte expand is always within HKDF limits");
            u32::from_be_bytes(okm)
        }
        #[cfg(not(feature = "crypto"))]
        {
            // FNV-1a, domain-separated so a token can never collide with a
            // truncated RoomId used for the same field.
            let mut hash: u32 = 0x811C_9DC5;
            for byte in b"anvil-room-token/v1".iter().chain(self.chars.iter()) {
                hash ^= u32::from(*byte);
                hash = hash.wrapping_mul(0x0100_0193);
            }
            hash
        }
    }
}

impl fmt::Display for JoinCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.formatted())
    }
}

impl fmt::Debug for JoinCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "JoinCode({})", self.formatted())
    }
}

/// A room's two identifiers, kept together so they cannot drift apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoomIdentity {
    /// Protocol identity. Random, and derived from nothing.
    pub room_id: RoomId,
    /// Human-facing code.
    pub join_code: JoinCode,
}

impl RoomIdentity {
    /// Generate both.
    ///
    /// They are independent on purpose. Deriving the `RoomId` from the join code
    /// would cap the room's identity at the code's 40 bits, which would make
    /// room ids guessable — and room ids appear in packet headers.
    #[must_use]
    pub fn generate() -> Self {
        Self { room_id: RoomId::generate(), join_code: JoinCode::generate() }
    }

    /// Token advertised for discovery.
    #[must_use]
    pub fn discovery_token(&self) -> u32 {
        self.join_code.discovery_token()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_codes_look_like_the_designed_format() {
        let code = JoinCode::generate();
        let text = code.formatted();

        assert!(text.starts_with("ANV-"), "{text}");
        assert_eq!(text.len(), 13, "{text}");
        assert_eq!(text.matches('-').count(), 2);
    }

    #[test]
    fn codes_round_trip() {
        for _ in 0..100 {
            let code = JoinCode::generate();
            assert_eq!(JoinCode::parse(&code.formatted()), Some(code));
            assert_eq!(JoinCode::parse(&code.raw()), Some(code));
        }
    }

    #[test]
    fn generated_codes_differ() {
        let codes: std::collections::HashSet<String> =
            (0..200).map(|_| JoinCode::generate().raw()).collect();
        assert!(codes.len() > 190, "only {} distinct codes in 200", codes.len());
    }

    #[test]
    fn parsing_forgives_how_people_actually_type() {
        let canonical = JoinCode::parse("ANV-7FK2-P9W4").unwrap();

        for variant in [
            "anv-7fk2-p9w4",
            "ANV7FK2P9W4",
            "7FK2-P9W4",
            "7fk2p9w4",
            "  ANV-7FK2-P9W4  ",
            "ANV 7FK2 P9W4",
            "anv_7fk2_p9w4",
        ] {
            assert_eq!(JoinCode::parse(variant), Some(canonical), "failed on {variant:?}");
        }
    }

    #[test]
    fn ambiguous_characters_are_corrected_rather_than_rejected() {
        // Someone reads "zero" aloud, the listener types the letter O.
        let with_zero = JoinCode::parse("ANV-70K2-P9W4").unwrap();
        let with_letter_o = JoinCode::parse("ANV-7OK2-P9W4").unwrap();
        assert_eq!(with_zero, with_letter_o);

        // Same for I/L and 1.
        let with_one = JoinCode::parse("ANV-71K2-P9W4").unwrap();
        assert_eq!(JoinCode::parse("ANV-7IK2-P9W4"), Some(with_one));
        assert_eq!(JoinCode::parse("ANV-7LK2-P9W4"), Some(with_one));
    }

    #[test]
    fn wrong_length_input_is_rejected() {
        for text in ["", "ANV", "ANV-7FK2", "7FK2P9W4XX", "ANV-7FK2-P9W4-EXTRA"] {
            assert!(JoinCode::parse(text).is_none(), "accepted {text:?}");
        }
    }

    #[test]
    fn the_alphabet_excludes_confusable_characters() {
        for confusable in [b'I', b'L', b'O', b'U'] {
            assert!(
                !ALPHABET.contains(&confusable),
                "{} should not be generatable",
                confusable as char
            );
        }
    }

    #[test]
    fn discovery_tokens_are_deterministic_across_devices() {
        // The host derives it; the joiner derives it from what they typed. If
        // these ever differ, nobody can join anything.
        let code = JoinCode::parse("ANV-7FK2-P9W4").unwrap();
        let typed_differently = JoinCode::parse("anv7fk2p9w4").unwrap();

        assert_eq!(code.discovery_token(), typed_differently.discovery_token());
    }

    #[test]
    fn different_codes_produce_different_tokens() {
        let tokens: std::collections::HashSet<u32> =
            (0..500).map(|_| JoinCode::generate().discovery_token()).collect();

        // A few collisions in 500 draws from 2^32 would be extraordinary.
        assert!(tokens.len() > 495, "only {} distinct tokens in 500", tokens.len());
    }

    #[test]
    fn room_id_is_not_derived_from_the_join_code() {
        // If it were, room ids would inherit the code's 40 bits — and room ids
        // appear in packet headers.
        let a = RoomIdentity::generate();
        let b = RoomIdentity::generate();

        assert_ne!(a.room_id, b.room_id);
        assert_ne!(a.join_code, b.join_code);

        // Two identities with the same code would still have different rooms.
        let shared_code = a.join_code;
        let first = RoomIdentity { room_id: RoomId::generate(), join_code: shared_code };
        let second = RoomIdentity { room_id: RoomId::generate(), join_code: shared_code };
        assert_ne!(first.room_id, second.room_id);
        assert_eq!(first.discovery_token(), second.discovery_token());
    }

    /// Pin the discovery_token to a known value so swapping the underlying
    /// hash function cannot silently change what every existing room advertises.
    /// The exact bytes here are not security-relevant — they are the value
    /// peers look for in the Wi-Fi Aware / LAN advertisement slots — but they
    /// *are* protocol-relevant: changing them would orphan every in-flight room.
    #[cfg(feature = "crypto")]
    #[test]
    fn discovery_token_is_stable_under_hkdf() {
        let code = JoinCode::parse("ANV-7FK2-P9W4").unwrap();
        assert_eq!(code.discovery_token(), 0xe4f1_9b4f);
    }

    /// Mirror test for the no-crypto fallback, so the scaffold build has its
    /// own stable advertisement value to ship with.
    #[cfg(not(feature = "crypto"))]
    #[test]
    fn discovery_token_is_stable_under_fnv1a() {
        let code = JoinCode::parse("ANV-7FK2-P9W4").unwrap();
        let mut expected: u32 = 0x811C_9DC5;
        for byte in b"anvil-room-token/v1".iter().chain(b"7FK2P9W4".iter()) {
            expected ^= u32::from(*byte);
            expected = expected.wrapping_mul(0x0100_0193);
        }
        assert_eq!(code.discovery_token(), expected);
    }
}
