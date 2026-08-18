//! Device identity (§45, §46).
//!
//! Every installation generates one long-lived Ed25519 keypair on first run.
//! The private half never leaves the device and, where the hardware allows,
//! never leaves secure storage either (§82). The public half is what other
//! devices know as "this phone".
//!
//! [`PeerId`] is the SHA-256 of the public key rather than the key itself. That
//! keeps ids fixed-width and signature-scheme-agnostic: moving to a different
//! primitive later changes how ids are computed, not what they are.
//!
//! ## What identity does and does not mean
//!
//! A verified identity proves *the same device as last time*. It proves nothing
//! about who is holding it, and Anvil has no directory, no accounts and no
//! authority to ask. Trust is established out of band — the participants are in
//! the same room and can see each other — and the UI should reflect that rather
//! than implying a verified name means a verified person.

use crate::{CryptoError, PeerId, Result};

use super::key_store::IdentityStore;

/// Length of the fingerprint carried in discovery advertisements.
pub type IdentityFingerprint = crate::discovery::Fingerprint;

/// A peer's public identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PublicIdentity {
    /// Ed25519 public key.
    pub key: [u8; 32],
}

impl PublicIdentity {
    /// Wrap a public key.
    #[must_use]
    pub const fn new(key: [u8; 32]) -> Self {
        Self { key }
    }

    /// The [`PeerId`] this key implies.
    ///
    /// PHASE2: real SHA-256 once the `crypto` feature is on. The placeholder
    /// below is *not* a hash and is not collision-resistant — it exists so the
    /// scaffold's types line up, and it is deliberately obvious rather than
    /// subtly wrong.
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        #[cfg(feature = "crypto")]
        {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(self.key);
            let mut id = [0u8; 32];
            id.copy_from_slice(&digest);
            PeerId(id)
        }
        #[cfg(not(feature = "crypto"))]
        {
            PeerId(self.key)
        }
    }

    /// The truncated fingerprint advertised during discovery.
    #[must_use]
    pub fn fingerprint(&self) -> IdentityFingerprint {
        let id = self.peer_id();
        let mut fp = [0u8; crate::discovery::FINGERPRINT_LEN];
        fp.copy_from_slice(&id.0[..crate::discovery::FINGERPRINT_LEN]);
        fp
    }

    /// Verify a signature made by this identity.
    ///
    /// PHASE2.
    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> Result<()> {
        #[cfg(feature = "crypto")]
        {
            let key = ed25519_dalek::VerifyingKey::from_bytes(&self.key)
                .map_err(|_| CryptoError::IdentityRejected)?;
            let signature = ed25519_dalek::Signature::from_bytes(signature);
            key.verify_strict(message, &signature).map_err(|_| CryptoError::IdentityRejected.into())
        }
        #[cfg(not(feature = "crypto"))]
        {
            let _ = (message, signature);
            Err(crate::Error::NotImplemented("crypto feature disabled"))
        }
    }
}

impl core::fmt::Debug for PublicIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "PublicIdentity({})", self.peer_id().short())
    }
}

/// This device's identity, private half included.
///
/// Never `Clone`. There is exactly one of these per process, and making it
/// copyable would make it easy to leave a second copy of the private key
/// somewhere it should not be.
pub struct DeviceIdentity {
    public: PublicIdentity,
    #[cfg(feature = "crypto")]
    private: ed25519_dalek::SigningKey,
    #[cfg(not(feature = "crypto"))]
    private: PrivateKeyPlaceholder,
}

#[cfg(not(feature = "crypto"))]
#[derive(zeroize::ZeroizeOnDrop)]
struct PrivateKeyPlaceholder([u8; 32]);

impl DeviceIdentity {
    /// Load the stored identity, generating one on first run.
    ///
    /// PHASE2: real keygen and storage. Note the shape though — the store is
    /// injected rather than reached for, so a test can run the whole identity
    /// lifecycle without touching a Keychain.
    pub fn load_or_generate(store: &dyn IdentityStore) -> Result<Self> {
        if let Some(stored) = store.load()? {
            if stored.len() < 32 {
                return Err(CryptoError::KeyStorage("stored identity is truncated".into()).into());
            }
            let mut secret = [0u8; 32];
            secret.copy_from_slice(&stored[..32]);
            return Self::from_secret(secret);
        }

        let identity = Self::generate();
        store.store(&identity.secret_bytes())?;
        Ok(identity)
    }

    /// Restore an identity from its 32-byte Ed25519 seed.
    pub fn from_secret(secret: [u8; 32]) -> Result<Self> {
        #[cfg(feature = "crypto")]
        {
            let private = ed25519_dalek::SigningKey::from_bytes(&secret);
            let public = PublicIdentity::new(private.verifying_key().to_bytes());
            Ok(Self { public, private })
        }
        #[cfg(not(feature = "crypto"))]
        {
            Ok(Self { public: PublicIdentity::new(secret), private: PrivateKeyPlaceholder(secret) })
        }
    }

    /// Generate a fresh installation identity with the operating-system RNG.
    #[must_use]
    pub fn generate() -> Self {
        #[cfg(feature = "crypto")]
        {
            let private = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
            let public = PublicIdentity::new(private.verifying_key().to_bytes());
            Self { public, private }
        }
        #[cfg(not(feature = "crypto"))]
        {
            let mut secret = [0u8; 32];
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut secret);
            Self { public: PublicIdentity::new(secret), private: PrivateKeyPlaceholder(secret) }
        }
    }

    /// This device's public identity.
    #[must_use]
    pub const fn public(&self) -> PublicIdentity {
        self.public
    }

    /// This device's [`PeerId`].
    #[must_use]
    pub fn peer_id(&self) -> PeerId {
        self.public.peer_id()
    }

    /// Sign a message.
    ///
    /// PHASE2.
    pub fn sign(&self, message: &[u8]) -> Result<[u8; 64]> {
        #[cfg(feature = "crypto")]
        {
            use ed25519_dalek::Signer;
            Ok(self.private.sign(message).to_bytes())
        }
        #[cfg(not(feature = "crypto"))]
        {
            let _ = message;
            Err(crate::Error::NotImplemented("crypto feature disabled"))
        }
    }

    /// Seed bytes for the platform keystore. Never expose across the UI FFI.
    pub(crate) fn secret_bytes(&self) -> [u8; 32] {
        #[cfg(feature = "crypto")]
        {
            self.private.to_bytes()
        }
        #[cfg(not(feature = "crypto"))]
        {
            self.private.0
        }
    }
}

impl core::fmt::Debug for DeviceIdentity {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The private half must never reach a log.
        write!(f, "DeviceIdentity({})", self.peer_id().short())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_a_prefix_of_the_peer_id() {
        let identity = PublicIdentity::new([0xAB; 32]);
        let id = identity.peer_id();
        assert_eq!(identity.fingerprint(), id.0[..8]);
    }

    #[test]
    fn different_keys_give_different_identities() {
        let a = PublicIdentity::new([1u8; 32]);
        let b = PublicIdentity::new([2u8; 32]);
        assert_ne!(a.peer_id(), b.peer_id());
    }

    #[test]
    fn debug_output_never_contains_raw_key_material() {
        let identity = PublicIdentity::new([0xCD; 32]);
        let rendered = format!("{identity:?}");
        assert!(!rendered.contains("cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"), "{rendered}");
    }
}
