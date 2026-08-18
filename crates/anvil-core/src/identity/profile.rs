//! The local profile — Anvil's replacement for an account.
//!
//! There is no signup, no login, no server, no directory. On first launch the
//! user types a display name; everything else is generated on the device and
//! stays there.
//!
//! ```text
//!   user types "Femi"
//!          ↓
//!   identity keypair            ← private half never leaves the device
//!   PeerId (from the public key)
//!   fingerprint
//!   created_at
//! ```
//!
//! ## What replaces what
//!
//! | Conventional | Anvil |
//! |---|---|
//! | account | a keypair |
//! | user id | `PeerId`, derived from the public key |
//! | login | loading the local profile |
//! | session token | nothing — peer sessions are networking, not auth |
//! | display name | a label, deliberately not an identity |
//!
//! ## The word "session"
//!
//! Worth being precise, because the two meanings pull in opposite directions.
//! The **local identity** is permanent: it survives restarts and lasts until the
//! user resets Anvil, clears app data, rotates deliberately, or uninstalls. A
//! **peer session** is an ephemeral cryptographic relationship with one other
//! device that dies with the connection. Nothing here is a login session,
//! because nothing here logs in.

use crate::time::Monotonic;
use crate::{PeerId, PROTOCOL_VERSION};

use super::{Fingerprint, PublicIdentity};

/// Longest display name accepted.
///
/// Bounded because it travels in the discovery advertisement, where the budget
/// is measured in tens of bytes.
pub const MAX_DISPLAY_NAME: usize = 48;

/// Why a display name was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NameError {
    /// Empty, or nothing but whitespace.
    Empty,
    /// Longer than [`MAX_DISPLAY_NAME`] bytes once trimmed.
    TooLong,
}

impl core::fmt::Display for NameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::Empty => "a display name is required",
            Self::TooLong => "that name is too long",
        })
    }
}

/// This device's identity, as the UI sees it.
///
/// Deliberately contains no private key. The private half lives behind
/// [`crate::crypto::DeviceIdentity`] and platform secure storage; nothing that
/// crosses the FFI boundary or reaches a screen should be able to hold it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalProfile {
    /// What humans call this device. Not an identity.
    pub display_name: String,
    /// The cryptographic identity.
    pub peer_id: PeerId,
    /// Public identity key.
    pub public_key: [u8; 32],
    /// When the identity was generated, in local monotonic time.
    ///
    /// Deliberately not wall-clock: a device with no internet has no reliable
    /// clock, and a "created" timestamp that jumps around is worse than one
    /// that is honestly relative. Wall-clock display is the host's problem.
    pub created_at: Monotonic,
    /// Wire version at creation.
    pub protocol_version: u8,
}

impl LocalProfile {
    /// Build a profile around an already-generated identity.
    ///
    /// Trims the name and validates it. The caller has already generated the
    /// keypair — this type never does, so there is exactly one place in the
    /// codebase where key generation happens.
    pub fn new(
        display_name: &str,
        identity: PublicIdentity,
        created_at: Monotonic,
    ) -> Result<Self, NameError> {
        let display_name = validate_name(display_name)?;

        Ok(Self {
            display_name,
            peer_id: identity.peer_id(),
            public_key: identity.key,
            created_at,
            protocol_version: PROTOCOL_VERSION,
        })
    }

    /// Fingerprint shown in the UI.
    #[must_use]
    pub fn fingerprint(&self) -> Fingerprint {
        Fingerprint::of(self.peer_id)
    }

    /// Change the display name.
    ///
    /// Cheap and local: it changes a label, not an identity. Peers who already
    /// know this device recognise it by key and simply see the new name.
    pub fn rename(&mut self, display_name: &str) -> Result<(), NameError> {
        self.display_name = validate_name(display_name)?;
        Ok(())
    }
}

/// Trim and check a display name.
pub fn validate_name(name: &str) -> Result<String, NameError> {
    let trimmed = name.trim();

    if trimmed.is_empty() {
        return Err(NameError::Empty);
    }
    if trimmed.len() > MAX_DISPLAY_NAME {
        return Err(NameError::TooLong);
    }
    Ok(trimmed.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(byte: u8) -> PublicIdentity {
        PublicIdentity::new([byte; 32])
    }

    #[test]
    fn a_profile_is_built_from_a_name_and_a_key() {
        let profile = LocalProfile::new("Femi", identity(0xAB), Monotonic(500)).unwrap();

        assert_eq!(profile.display_name, "Femi");
        assert_eq!(profile.peer_id, identity(0xAB).peer_id());
        assert_eq!(profile.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn names_are_trimmed() {
        let profile = LocalProfile::new("  Femi  ", identity(1), Monotonic::ZERO).unwrap();
        assert_eq!(profile.display_name, "Femi");
    }

    #[test]
    fn empty_names_are_rejected_so_first_run_cannot_be_skipped_blank() {
        assert_eq!(validate_name(""), Err(NameError::Empty));
        assert_eq!(validate_name("   "), Err(NameError::Empty));
        assert_eq!(validate_name("\t\n"), Err(NameError::Empty));
    }

    #[test]
    fn overlong_names_are_rejected_because_they_must_fit_an_advertisement() {
        let long = "n".repeat(MAX_DISPLAY_NAME + 1);
        assert_eq!(validate_name(&long), Err(NameError::TooLong));
        assert!(validate_name(&"n".repeat(MAX_DISPLAY_NAME)).is_ok());
    }

    #[test]
    fn two_people_may_share_a_name_and_remain_different_identities() {
        // The point of separating name from identity.
        let a = LocalProfile::new("Femi", identity(0xA1), Monotonic::ZERO).unwrap();
        let b = LocalProfile::new("Femi", identity(0xC3), Monotonic::ZERO).unwrap();

        assert_eq!(a.display_name, b.display_name);
        assert_ne!(a.peer_id, b.peer_id);
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn renaming_keeps_the_identity() {
        let mut profile = LocalProfile::new("Femi", identity(7), Monotonic::ZERO).unwrap();
        let peer_id = profile.peer_id;

        profile.rename("Femi A.").unwrap();

        assert_eq!(profile.display_name, "Femi A.");
        assert_eq!(profile.peer_id, peer_id, "renaming must not change identity");
    }

    #[test]
    fn a_profile_never_carries_a_private_key() {
        // Structural: there is no field for one. If a future change adds one,
        // this comment is the reason to push back — profiles cross the FFI
        // boundary and reach screens.
        let profile = LocalProfile::new("Femi", identity(9), Monotonic::ZERO).unwrap();
        let rendered = format!("{profile:?}");
        assert!(!rendered.to_lowercase().contains("private"), "{rendered}");
    }
}
