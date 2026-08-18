//! Session establishment (§47, §67).
//!
//! ```text
//!   Hello      ──►   version check, fingerprint
//!   Identity   ◄─►   Ed25519 identity + X25519 ephemeral + signature
//!                    over a transcript that includes both nonces
//!   ───────────────  session keys via HKDF-SHA-256
//!   RoomJoin   ──►   admission
//!   KeyExchange ◄─►  per-sender media keys for the current epoch
//! ```
//!
//! Three properties this has to have, and the reason each one is not optional:
//!
//! * **Ephemeral session keys, separate from identity.** A compromised identity
//!   key must not decrypt yesterday's recorded call. X25519 ephemerals give
//!   forward secrecy for the session; the identity key only authenticates.
//! * **Transcript binding.** The signature covers both sides' nonces and the
//!   negotiated version, so a captured handshake cannot be replayed against a
//!   different session, and a downgrade cannot be forced by an attacker
//!   rewriting the version byte.
//! * **Rejection is terminal.** A failed handshake tears the path down. There
//!   is no retry-with-less: an attacker who can make verification fail should
//!   get a closed connection, not a weaker one.
//!
//! ## Phase status
//!
//! PHASE2. The state machine below is real and the transitions are enforced;
//! the cryptographic steps are stubs. That ordering is on purpose — the state
//! machine is where handshake bugs actually live, and having it testable before
//! any key material exists is worth more than the reverse.

use crate::{CryptoError, ProtocolError, Result};

/// Where a handshake has got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeState {
    /// Nothing sent yet.
    Idle,
    /// `Hello` sent, waiting for theirs.
    HelloSent,
    /// Versions agreed, identity exchange in flight.
    IdentityExchange,
    /// Both identities verified, session keys derived.
    Established,
    /// Failed. Terminal — the path is torn down.
    Failed,
}

impl HandshakeState {
    /// Whether the session can carry application traffic.
    #[must_use]
    pub const fn is_established(self) -> bool {
        matches!(self, Self::Established)
    }

    /// Whether this state can never progress.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Established | Self::Failed)
    }
}

/// Handshake for one path.
#[derive(Debug)]
pub struct Handshake {
    state: HandshakeState,
    /// Version agreed with the peer, once known.
    negotiated_version: Option<u8>,
    /// Whether this side opened the connection. Decides who speaks first and,
    /// where a tie has to be broken, whose value wins.
    initiator: bool,
}

impl Handshake {
    /// Start a handshake.
    #[must_use]
    pub fn new(initiator: bool) -> Self {
        Self { state: HandshakeState::Idle, negotiated_version: None, initiator }
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> HandshakeState {
        self.state
    }

    /// Agreed protocol version, once negotiated.
    #[must_use]
    pub const fn negotiated_version(&self) -> Option<u8> {
        self.negotiated_version
    }

    /// Whether this side initiated.
    #[must_use]
    pub const fn is_initiator(&self) -> bool {
        self.initiator
    }

    /// Record that our `Hello` went out.
    pub fn on_hello_sent(&mut self) -> Result<()> {
        self.expect(HandshakeState::Idle, "Hello sent")?;
        self.state = HandshakeState::HelloSent;
        Ok(())
    }

    /// Handle the peer's `Hello`.
    ///
    /// A version mismatch fails the handshake immediately rather than
    /// continuing and hoping.
    pub fn on_hello_received(&mut self, their_version: u8) -> Result<u8> {
        if self.state.is_terminal() {
            return Err(ProtocolError::UnexpectedMessage {
                message: "Hello",
                state: "terminal",
            }
            .into());
        }

        match crate::protocol::negotiate(their_version) {
            Ok(version) => {
                self.negotiated_version = Some(version);
                self.state = HandshakeState::IdentityExchange;
                Ok(version)
            }
            Err(e) => {
                self.state = HandshakeState::Failed;
                Err(e)
            }
        }
    }

    /// Handle the peer's identity.
    ///
    /// PHASE2: verify the signature over the transcript, derive session keys
    /// via X25519 + HKDF-SHA-256, and confirm the peer id matches what
    /// discovery advertised.
    pub fn on_identity_received(&mut self) -> Result<()> {
        self.expect(HandshakeState::IdentityExchange, "Identity")?;
        self.state = HandshakeState::Failed;
        Err(CryptoError::HandshakeFailed("identity verification not implemented (Phase 2)").into())
    }

    /// Abandon the handshake.
    pub fn fail(&mut self) {
        self.state = HandshakeState::Failed;
    }

    fn expect(&self, wanted: HandshakeState, message: &'static str) -> Result<()> {
        if self.state == wanted {
            return Ok(());
        }
        Err(ProtocolError::UnexpectedMessage {
            message,
            state: match self.state {
                HandshakeState::Idle => "Idle",
                HandshakeState::HelloSent => "HelloSent",
                HandshakeState::IdentityExchange => "IdentityExchange",
                HandshakeState::Established => "Established",
                HandshakeState::Failed => "Failed",
            },
        }
        .into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progresses_through_the_expected_states() {
        let mut hs = Handshake::new(true);
        assert_eq!(hs.state(), HandshakeState::Idle);

        hs.on_hello_sent().unwrap();
        assert_eq!(hs.state(), HandshakeState::HelloSent);

        hs.on_hello_received(crate::PROTOCOL_VERSION).unwrap();
        assert_eq!(hs.state(), HandshakeState::IdentityExchange);
        assert_eq!(hs.negotiated_version(), Some(crate::PROTOCOL_VERSION));
    }

    #[test]
    fn a_version_mismatch_fails_terminally() {
        let mut hs = Handshake::new(false);
        hs.on_hello_sent().unwrap();

        assert!(hs.on_hello_received(99).is_err());
        assert_eq!(hs.state(), HandshakeState::Failed);
        assert!(hs.state().is_terminal());

        // No second chance, no downgrade.
        assert!(hs.on_hello_received(crate::PROTOCOL_VERSION).is_err());
    }

    #[test]
    fn messages_out_of_order_are_rejected() {
        let mut hs = Handshake::new(true);
        // Identity before any Hello.
        assert!(hs.on_identity_received().is_err());
    }

    #[test]
    fn hello_cannot_be_sent_twice() {
        let mut hs = Handshake::new(true);
        hs.on_hello_sent().unwrap();
        assert!(hs.on_hello_sent().is_err());
    }

    #[test]
    fn failure_is_sticky() {
        let mut hs = Handshake::new(true);
        hs.fail();
        assert!(hs.on_hello_sent().is_err());
        assert!(!hs.state().is_established());
    }
}
