//! Cryptography (§42–§54).
//!
//! Two layers, protecting against two different adversaries. Confusing them is
//! the single most common way an end-to-end encrypted system turns out not to
//! be one.
//!
//! ```text
//!   ┌────────────────────────────────────────────────┐
//!   │ End-to-end media encryption                    │
//!   │   Adversary: the relay, and anyone on the path │
//!   │   Terminated at: the two endpoints             │
//!   ├────────────────────────────────────────────────┤
//!   │ Transport security (QUIC/TLS)                  │
//!   │   Adversary: passive/active attackers on a hop │
//!   │   Terminated at: each hop, including the relay │
//!   └────────────────────────────────────────────────┘
//! ```
//!
//! The relay terminates QUIC. If media were only protected by transport
//! security, the relay would see plaintext — which is exactly the architecture
//! §42 rejects. The media envelope is encrypted before it is handed to the
//! transport and decrypted only by endpoints holding the sender's key.
//!
//! ## Rules
//!
//! * **No invented primitives** (§44). Ed25519, X25519, HKDF-SHA-256,
//!   ChaCha20-Poly1305, via reviewed Rust crates.
//! * **Authenticate before decoding** (§80). A packet that fails its tag is
//!   discarded and never reaches the Opus decoder — the decoder is a large
//!   surface that must only ever see bytes a room member authored.
//! * **Never log key material** (§92). No `Debug` impl in this module prints a
//!   secret; secrets zeroize on drop.
//!
//! ## Group keys and the MLS seam
//!
//! v0.1 uses per-sender media keys (§49) rather than MLS, which is a real
//! trade-off: sender keys mean a membership change costs O(members²) key
//! deliveries, and forward secrecy is only as good as the epoch rotation
//! discipline. MLS fixes both and is a great deal more work.
//!
//! The compromise §51 asks for is honoured here: everything above this module
//! talks to [`GroupKeyManager`], never to sender-key internals. Swapping in an
//! MLS implementation should touch this module and nothing else.

#[cfg(feature = "crypto")]
pub mod control;
pub mod epoch;
pub mod handshake;
pub mod identity;
pub mod key_store;
pub mod media;
#[cfg(feature = "crypto")]
pub mod sender_key;
#[cfg(feature = "crypto")]
pub mod session;

#[cfg(feature = "crypto")]
pub use control::{AppControl, SecureControl};
pub use epoch::EpochManager;
pub use handshake::{Handshake, HandshakeState};
pub use identity::{DeviceIdentity, IdentityFingerprint, PublicIdentity};
pub use key_store::IdentityStore;
pub use media::{MediaKey, ReplayWindow, NONCE_LEN};
#[cfg(feature = "crypto")]
pub use sender_key::SenderKeyManager;
#[cfg(feature = "crypto")]
pub use session::{EstablishedSession, SessionHandshake};

#[cfg(feature = "crypto")]
use crate::{Epoch, PeerId, Result};

/// The seam between the protocol and whatever group key scheme is in use.
///
/// v0.1 implements this with [`SenderKeyManager`]. A future MLS implementation
/// implements the same trait, and the media pipeline does not change.
///
/// The trait is written in terms of *what the protocol needs* — seal a frame
/// for the room, open a frame from a member, advance on membership change —
/// rather than in terms of keys, so that a scheme with a different key
/// structure can satisfy it without contortion.
#[cfg(feature = "crypto")]
pub trait GroupKeyManager: Send + core::fmt::Debug {
    /// Current epoch.
    fn epoch(&self) -> Epoch;

    /// Encrypt one media frame for the room.
    ///
    /// `associated_data` is the packet header, so the visible routing fields
    /// are authenticated even though they are not encrypted — a relay can read
    /// them but cannot alter them undetected.
    fn seal(&mut self, plaintext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>>;

    /// Decrypt one media frame from `sender`.
    ///
    /// Must reject, in this order: unknown epoch, replayed sequence, failed
    /// authentication. Returning distinguishable errors to a *peer* would be an
    /// oracle; returning them locally is how the diagnostics view stays useful.
    fn open(
        &mut self,
        sender: PeerId,
        epoch: Epoch,
        sequence: crate::SeqNum,
        ciphertext: &[u8],
        associated_data: &[u8],
    ) -> Result<Vec<u8>>;

    /// Advance to a new epoch after a membership change (§50).
    ///
    /// Implementations must destroy key material for epochs older than the
    /// retention window, or departure means nothing.
    fn rotate(&mut self, new_epoch: Epoch, members: &[PeerId]) -> Result<()>;

    /// Accept a member's key material for an epoch, received over an
    /// authenticated session.
    fn install_member_key(&mut self, member: PeerId, epoch: Epoch, key: &[u8]) -> Result<()>;

    /// Forget a member's key material entirely.
    fn remove_member(&mut self, member: PeerId);
}
