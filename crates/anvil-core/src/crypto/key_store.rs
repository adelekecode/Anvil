//! Long-term key storage (§82).
//!
//! The device identity key is the one secret in Anvil that outlives the
//! process. Everything else — session keys, media keys — is ephemeral and dies
//! with the room.
//!
//! Where the platform offers it, the private key should live in
//! hardware-backed storage (iOS Keychain with Secure Enclave, Android Keystore
//! with StrongBox or TEE) and be *used* there rather than exported. Where it is
//! not available, the key is stored in the platform's software keystore and
//! [`crate::platform::Capabilities::secure_key_storage`] reports false, so the
//! diagnostics view can say so honestly rather than implying a guarantee the
//! device is not providing.
//!
//! This trait is a thin wrapper over
//! [`crate::platform::KeyStoreAdapter`] that exists so identity code depends on
//! a narrow interface it can fake in tests, rather than on the whole platform.

use crate::Result;

/// Persistent storage for the device identity key.
pub trait IdentityStore: Send + Sync + core::fmt::Debug {
    /// Load the stored identity, or `None` on first run.
    fn load(&self) -> Result<Option<Vec<u8>>>;

    /// Persist the identity.
    fn store(&self, bytes: &[u8]) -> Result<()>;

    /// Destroy the stored identity.
    ///
    /// Irreversible: the device becomes a stranger to every peer that has met
    /// it. Should be wired to an explicit, confirmed user action and nothing
    /// else.
    fn clear(&self) -> Result<()>;
}

impl<T: crate::platform::KeyStoreAdapter> IdentityStore for T {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        self.load_identity()
    }

    fn store(&self, bytes: &[u8]) -> Result<()> {
        self.store_identity(bytes)
    }

    fn clear(&self) -> Result<()> {
        self.clear_identity()
    }
}

/// In-memory store for tests. Never for a real device — it forgets the identity
/// on every restart, which would make a phone a new peer every launch.
#[derive(Debug, Default)]
pub struct MemoryStore {
    bytes: std::sync::Mutex<Option<Vec<u8>>>,
}

impl MemoryStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl IdentityStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>> {
        Ok(self.bytes.lock().expect("store poisoned").clone())
    }

    fn store(&self, bytes: &[u8]) -> Result<()> {
        *self.bytes.lock().expect("store poisoned") = Some(bytes.to_vec());
        Ok(())
    }

    fn clear(&self) -> Result<()> {
        *self.bytes.lock().expect("store poisoned") = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_store_round_trips() {
        let store = MemoryStore::new();
        assert_eq!(store.load().unwrap(), None);

        store.store(&[1, 2, 3]).unwrap();
        assert_eq!(store.load().unwrap(), Some(vec![1, 2, 3]));

        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);
    }
}
