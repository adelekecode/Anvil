//! Authenticated path handshake carried on QUIC reliable streams.

use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use sha2::Sha256;
use x25519_dalek::{PublicKey as X25519PublicKey, StaticSecret};

use super::{DeviceIdentity, PublicIdentity};
use crate::discovery::Fingerprint;
use crate::{CryptoError, PeerId, ProtocolError, Result, PROTOCOL_VERSION};

const MAGIC: &[u8; 4] = b"ANV1";
const HELLO: u8 = 1;
const IDENTITY: u8 = 2;
const TRANSCRIPT_DOMAIN: &[u8] = b"anvil-session-v1";

/// Authenticated result of one path handshake.
#[derive(Debug)]
pub struct EstablishedSession {
    /// Verified remote identity.
    pub peer_id: PeerId,
    /// Full verified Ed25519 public key.
    pub public_key: [u8; 32],
    /// Authenticated self-chosen name.
    pub display_name: String,
    /// Forward-secret path session key derived with X25519 + HKDF-SHA-256.
    pub session_key: [u8; 32],
}

/// State for one provisional QUIC path.
pub struct SessionHandshake {
    expected_fingerprint: Fingerprint,
    local_nonce: [u8; 32],
    remote_nonce: Option<[u8; 32]>,
    ephemeral: StaticSecret,
    established: bool,
}

impl core::fmt::Debug for SessionHandshake {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SessionHandshake")
            .field("expected_fingerprint", &self.expected_fingerprint)
            .field("has_remote_nonce", &self.remote_nonce.is_some())
            .field("established", &self.established)
            .finish()
    }
}

impl SessionHandshake {
    /// Start a challenge exchange for the fingerprint seen during discovery.
    #[must_use]
    pub fn new(expected_fingerprint: Fingerprint) -> Self {
        let mut local_nonce = [0u8; 32];
        OsRng.fill_bytes(&mut local_nonce);
        Self {
            expected_fingerprint,
            local_nonce,
            remote_nonce: None,
            ephemeral: StaticSecret::random_from_rng(OsRng),
            established: false,
        }
    }

    /// Initial record sent as soon as QUIC establishes.
    #[must_use]
    pub fn hello(&self, identity: &DeviceIdentity) -> Vec<u8> {
        let mut out = Vec::with_capacity(46);
        out.extend_from_slice(MAGIC);
        out.push(HELLO);
        out.push(PROTOCOL_VERSION);
        out.extend_from_slice(&identity.public().fingerprint());
        out.extend_from_slice(&self.local_nonce);
        out
    }

    /// Process a peer challenge and return our signed identity response.
    pub fn receive_hello(
        &mut self,
        record: &[u8],
        identity: &DeviceIdentity,
        display_name: &str,
    ) -> Result<Vec<u8>> {
        if record.len() != 46 || &record[..4] != MAGIC || record[4] != HELLO {
            return Err(ProtocolError::Malformed("invalid session Hello").into());
        }
        if record[5] != PROTOCOL_VERSION {
            return Err(ProtocolError::VersionMismatch {
                theirs: record[5],
                ours: PROTOCOL_VERSION,
            }
            .into());
        }
        if record[6..14] != self.expected_fingerprint {
            return Err(CryptoError::HandshakeFailed("discovery fingerprint mismatch").into());
        }
        let mut remote_nonce = [0u8; 32];
        remote_nonce.copy_from_slice(&record[14..46]);
        self.remote_nonce = Some(remote_nonce);

        let public_key = identity.public().key;
        let ephemeral_key = X25519PublicKey::from(&self.ephemeral).to_bytes();
        let name = display_name.as_bytes();
        let name = &name[..name.len().min(48)];
        let transcript =
            transcript(&self.local_nonce, &remote_nonce, &public_key, &ephemeral_key, name);
        let signature = identity.sign(&transcript)?;

        let mut out = Vec::with_capacity(198 + name.len());
        out.extend_from_slice(MAGIC);
        out.push(IDENTITY);
        out.extend_from_slice(&public_key);
        out.extend_from_slice(&ephemeral_key);
        out.extend_from_slice(&self.local_nonce);
        out.extend_from_slice(&remote_nonce);
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        out.extend_from_slice(&signature);
        Ok(out)
    }

    /// Verify the peer response and derive the forward-secret session key.
    pub fn receive_identity(&mut self, record: &[u8]) -> Result<EstablishedSession> {
        const FIXED: usize = 4 + 1 + 32 + 32 + 32 + 32 + 1 + 64;
        if record.len() < FIXED || &record[..4] != MAGIC || record[4] != IDENTITY {
            return Err(ProtocolError::Malformed("invalid session Identity").into());
        }
        let name_len = record[133] as usize;
        if name_len > 48 || record.len() != FIXED + name_len {
            return Err(ProtocolError::Malformed("invalid identity name length").into());
        }

        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&record[5..37]);
        let public = PublicIdentity::new(public_key);
        if public.fingerprint() != self.expected_fingerprint {
            return Err(CryptoError::HandshakeFailed("identity does not match discovery").into());
        }
        let mut ephemeral_key = [0u8; 32];
        ephemeral_key.copy_from_slice(&record[37..69]);
        let mut sender_nonce = [0u8; 32];
        sender_nonce.copy_from_slice(&record[69..101]);
        let mut echoed_nonce = [0u8; 32];
        echoed_nonce.copy_from_slice(&record[101..133]);
        if echoed_nonce != self.local_nonce || Some(sender_nonce) != self.remote_nonce {
            return Err(CryptoError::HandshakeFailed("session challenge mismatch").into());
        }
        let name_end = 134 + name_len;
        let name_bytes = &record[134..name_end];
        let display_name = std::str::from_utf8(name_bytes)
            .map_err(|_| ProtocolError::Malformed("identity name is not UTF-8"))?
            .to_owned();
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&record[name_end..]);
        public.verify(
            &transcript(&sender_nonce, &self.local_nonce, &public_key, &ephemeral_key, name_bytes),
            &signature,
        )?;

        let shared = self.ephemeral.diffie_hellman(&X25519PublicKey::from(ephemeral_key));
        let mut salt = [0u8; 64];
        let (first, second) = if sender_nonce <= self.local_nonce {
            (&sender_nonce, &self.local_nonce)
        } else {
            (&self.local_nonce, &sender_nonce)
        };
        salt[..32].copy_from_slice(first);
        salt[32..].copy_from_slice(second);
        let mut session_key = [0u8; 32];
        Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes())
            .expand(TRANSCRIPT_DOMAIN, &mut session_key)
            .map_err(|_| CryptoError::HandshakeFailed("session key derivation failed"))?;
        self.established = true;

        Ok(EstablishedSession { peer_id: public.peer_id(), public_key, display_name, session_key })
    }

    /// Identify a record before parsing its type-specific fields.
    #[must_use]
    pub fn record_type(record: &[u8]) -> Option<u8> {
        (record.len() >= 5 && &record[..4] == MAGIC).then_some(record[4])
    }

    /// Hello record discriminator.
    pub const HELLO_RECORD: u8 = HELLO;
    /// Identity record discriminator.
    pub const IDENTITY_RECORD: u8 = IDENTITY;
}

fn transcript(
    sender_nonce: &[u8; 32],
    receiver_nonce: &[u8; 32],
    public_key: &[u8; 32],
    ephemeral_key: &[u8; 32],
    name: &[u8],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(TRANSCRIPT_DOMAIN.len() + 129 + name.len());
    out.extend_from_slice(TRANSCRIPT_DOMAIN);
    out.extend_from_slice(sender_nonce);
    out.extend_from_slice(receiver_nonce);
    out.extend_from_slice(public_key);
    out.extend_from_slice(ephemeral_key);
    out.push(name.len() as u8);
    out.extend_from_slice(name);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_installations_authenticate_and_derive_the_same_key() {
        let alice = DeviceIdentity::generate();
        let bob = DeviceIdentity::generate();
        let mut a = SessionHandshake::new(bob.public().fingerprint());
        let mut b = SessionHandshake::new(alice.public().fingerprint());

        let a_identity = b.receive_hello(&a.hello(&alice), &bob, "Bob").unwrap();
        let b_identity = a.receive_hello(&b.hello(&bob), &alice, "Alice").unwrap();
        let at_b = b.receive_identity(&b_identity).unwrap();
        let at_a = a.receive_identity(&a_identity).unwrap();

        assert_eq!(at_a.peer_id, bob.peer_id());
        assert_eq!(at_b.peer_id, alice.peer_id());
        assert_eq!(at_a.session_key, at_b.session_key);
    }

    #[test]
    fn copied_advertisement_cannot_authenticate_a_different_key() {
        let alice = DeviceIdentity::generate();
        let attacker = DeviceIdentity::generate();
        let mut victim = SessionHandshake::new(alice.public().fingerprint());
        assert!(victim
            .receive_hello(
                &SessionHandshake::new(attacker.public().fingerprint()).hello(&attacker),
                &alice,
                "Alice"
            )
            .is_err());
    }
}
