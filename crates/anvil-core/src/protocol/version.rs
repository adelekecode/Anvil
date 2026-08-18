//! Wire version handling.
//!
//! Anvil has no update server and no way to tell a user "everyone please
//! upgrade". Two phones in a field will be running whatever they were last
//! updated to, and a version mismatch has to fail in a way somebody can
//! understand from the UI.
//!
//! So the rule is: **refuse clearly, never half-support**. A peer speaking a
//! version this build does not know is rejected at the handshake with a
//! specific error, not accepted and then fed packets it will misparse.

use crate::{ProtocolError, Result, PROTOCOL_VERSION};

/// Versions this build can talk to.
///
/// A range rather than a single value so that adding v2 later does not
/// immediately partition every deployed device — a v2 build can keep speaking
/// v1 to older peers for a release or two.
pub const SUPPORTED: &[u8] = &[1];

/// Whether this build can speak `version`.
#[must_use]
pub fn is_supported(version: u8) -> bool {
    SUPPORTED.contains(&version)
}

/// Pick the version to use with a peer advertising `theirs`.
///
/// Returns the highest version both sides support.
pub fn negotiate(theirs: u8) -> Result<u8> {
    if is_supported(theirs) {
        Ok(theirs.min(PROTOCOL_VERSION))
    } else {
        Err(ProtocolError::VersionMismatch { theirs, ours: PROTOCOL_VERSION }.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_its_own_version() {
        assert_eq!(negotiate(PROTOCOL_VERSION).unwrap(), PROTOCOL_VERSION);
    }

    #[test]
    fn refuses_unknown_versions_rather_than_guessing() {
        assert!(negotiate(0).is_err());
        assert!(negotiate(2).is_err());
        assert!(negotiate(255).is_err());
    }
}
