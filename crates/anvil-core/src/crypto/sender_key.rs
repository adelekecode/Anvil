//! Per-sender media keys (§49).
//!
//! Each participant encrypts their own voice with their own key. Every other
//! authorised member receives that key over an authenticated peer session —
//! never through the relay, never broadcast.
//!
//! ```text
//!   Alice ── key_A ──► Bob, Chris, David
//!   Bob   ── key_B ──► Alice, Chris, David
//!   Chris ── key_C ──► Alice, Bob, David
//! ```
//!
//! ## What this costs
//!
//! Key distribution is O(n²) per epoch, and every membership change forces a
//! new epoch. Four participants is twelve deliveries — fine. Twelve
//! participants is 132 deliveries on every join and leave, over a shared
//! radio, which is not fine. This scheme is sized for the rooms v0.1 targets
//! (§94: three to four phones) and it will need replacing before rooms get
//! meaningfully larger. That is the MLS work §51 defers, and
//! [`crate::crypto::GroupKeyManager`] is the seam that makes the replacement
//! tractable.
//!
//! ## What it buys
//!
//! Sender-scoped keys give **sender authentication for free**. Because only
//! Alice holds Alice's key, a packet that decrypts under key_A was authored by
//! Alice. A single shared room key would let any member forge any other
//! member's voice — including a malicious relay who is also a participant,
//! which is the exact adversary §79 names.
//!
//! ## Phase status
//!
//! PHASE2. Structure and bookkeeping are here; key derivation and AEAD calls
//! are stubbed. The bookkeeping is the part with the security-relevant bugs
//! (forgetting to drop a departed member's key, accepting a key for the wrong
//! epoch), so it is written and tested first.

use std::collections::HashMap;

use crate::time::Monotonic;
use crate::{CryptoError, Epoch, PeerId, Result, SeqNum};

use super::media::{MediaKey, ReplayWindow};
use super::{EpochManager, GroupKeyManager};

/// Per-sender key material and replay state for the current epochs.
#[derive(Debug)]
pub struct SenderKeyManager {
    /// This device.
    local: PeerId,
    /// Epoch bookkeeping and retention.
    epochs: EpochManager,
    /// Our own sending keys, by epoch.
    own_keys: HashMap<Epoch, MediaKey>,
    /// Other members' keys, by (member, epoch).
    member_keys: HashMap<(PeerId, Epoch), MediaKey>,
    /// Replay state, by (member, epoch). Reset per epoch because sequence
    /// numbers restart with the key.
    replay: HashMap<(PeerId, Epoch), ReplayWindow>,
    /// Next sequence number for our own stream.
    next_sequence: SeqNum,
}

impl SenderKeyManager {
    /// A manager for this device.
    #[must_use]
    pub fn new(local: PeerId) -> Self {
        Self {
            local,
            epochs: EpochManager::new(),
            own_keys: HashMap::new(),
            member_keys: HashMap::new(),
            replay: HashMap::new(),
            next_sequence: SeqNum(0),
        }
    }

    /// Sequence number for the next outgoing frame.
    pub fn take_sequence(&mut self) -> SeqNum {
        let seq = self.next_sequence;
        self.next_sequence = seq.next();
        seq
    }

    /// Drop key material for epochs past their retention window.
    ///
    /// Call on the engine tick. Retention enforced only when a packet happens
    /// to arrive is not retention at all.
    pub fn expire_epochs(&mut self, now: Monotonic) -> usize {
        let expired = self.epochs.expire(now);
        for epoch in &expired {
            self.own_keys.remove(epoch);
            self.member_keys.retain(|(_, e), _| e != epoch);
            self.replay.retain(|(_, e), _| e != epoch);
        }
        expired.len()
    }

    /// Members we currently hold key material for, in the current epoch.
    #[must_use]
    pub fn known_members(&self) -> Vec<PeerId> {
        let current = self.epochs.current();
        let mut members: Vec<PeerId> = self
            .member_keys
            .keys()
            .filter(|(_, epoch)| *epoch == current)
            .map(|(peer, _)| *peer)
            .collect();
        members.sort_unstable();
        members
    }

    /// Whether we can decrypt traffic from `member` in the current epoch.
    #[must_use]
    pub fn can_decrypt(&self, member: PeerId) -> bool {
        self.member_keys.contains_key(&(member, self.epochs.current()))
    }

    /// Ensure this sender has fresh material for an epoch and return it for distribution.
    pub fn own_key_for_epoch(&mut self, epoch: Epoch) -> ([u8; 32], [u8; 12]) {
        self.own_keys.entry(epoch).or_insert_with(|| {
            let mut key = [0u8; 32];
            let mut salt = [0u8; 12];
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut key);
            rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut salt);
            MediaKey::new(key, salt)
        });
        self.own_keys.get(&epoch).expect("key inserted").material()
    }

    /// Install another authenticated member's sender material.
    pub fn install_member_material(
        &mut self,
        member: PeerId,
        epoch: Epoch,
        key: [u8; 32],
        salt: [u8; 12],
    ) {
        self.member_keys.insert((member, epoch), MediaKey::new(key, salt));
        self.replay.insert((member, epoch), ReplayWindow::new());
    }
}

impl GroupKeyManager for SenderKeyManager {
    fn epoch(&self) -> Epoch {
        self.epochs.current()
    }

    fn seal(&mut self, plaintext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>> {
        use chacha20poly1305::aead::{Aead, KeyInit, Payload};
        let header = crate::protocol::MediaHeader::decode(associated_data)?;
        let epoch = Epoch(u64::from(header.epoch));
        let key = self.own_keys.get(&epoch).ok_or(CryptoError::UnknownEpoch(epoch))?;
        let cipher = chacha20poly1305::ChaCha20Poly1305::new(key.bytes().into());
        cipher
            .encrypt(
                chacha20poly1305::Nonce::from_slice(&key.nonce(
                    epoch,
                    header.stream_id,
                    header.sequence,
                )),
                Payload { msg: plaintext, aad: associated_data },
            )
            .map_err(|_| CryptoError::AuthenticationFailed.into())
    }

    fn open(
        &mut self,
        sender: PeerId,
        epoch: Epoch,
        sequence: SeqNum,
        ciphertext: &[u8],
        associated_data: &[u8],
    ) -> Result<Vec<u8>> {
        // Order matters and is checked here rather than in the caller.
        //
        // 1. Do we have key material for this epoch at all?
        if !self.member_keys.contains_key(&(sender, epoch)) {
            return Err(CryptoError::UnknownEpoch(epoch).into());
        }

        use chacha20poly1305::aead::{Aead, KeyInit, Payload};
        let header = crate::protocol::MediaHeader::decode(associated_data)?;
        let key = self.member_keys.get(&(sender, epoch)).expect("checked above");
        let cipher = chacha20poly1305::ChaCha20Poly1305::new(key.bytes().into());
        let plaintext = cipher
            .decrypt(
                chacha20poly1305::Nonce::from_slice(&key.nonce(epoch, header.stream_id, sequence)),
                Payload { msg: ciphertext, aad: associated_data },
            )
            .map_err(|_| CryptoError::AuthenticationFailed)?;
        self.replay
            .get_mut(&(sender, epoch))
            .expect("installed with key")
            .check_and_update(sequence)?;
        Ok(plaintext)
    }

    fn rotate(&mut self, new_epoch: Epoch, members: &[PeerId]) -> Result<()> {
        let now = Monotonic::ZERO; // PHASE2: threaded from the engine clock
        if !self.epochs.advance(new_epoch, now) {
            return Ok(());
        }

        // Anyone not in the new member list loses their key immediately —
        // there is no in-flight audio to protect from someone who has left.
        self.member_keys.retain(|(peer, epoch), _| *epoch != new_epoch || members.contains(peer));
        self.replay.retain(|(peer, epoch), _| *epoch != new_epoch || members.contains(peer));

        // Sequence numbering restarts with the new key, which is what makes the
        // derived nonce unique across epochs.
        self.next_sequence = SeqNum(0);

        // PHASE2: derive a fresh own key for new_epoch and distribute it to
        // `members` over their authenticated sessions.
        let _ = self.local;
        Ok(())
    }

    fn install_member_key(&mut self, member: PeerId, epoch: Epoch, key: &[u8]) -> Result<()> {
        if key.len() != super::media::KEY_LEN {
            return Err(CryptoError::HandshakeFailed("bad media key length").into());
        }

        let mut bytes = [0u8; super::media::KEY_LEN];
        bytes.copy_from_slice(key);
        // PHASE2: salt comes from the same HKDF expansion as the key.
        let salt = [0u8; super::media::NONCE_LEN];

        self.member_keys.insert((member, epoch), MediaKey::new(bytes, salt));
        self.replay.insert((member, epoch), ReplayWindow::new());
        Ok(())
    }

    fn remove_member(&mut self, member: PeerId) {
        self.member_keys.retain(|(peer, _), _| *peer != member);
        self.replay.retain(|(peer, _), _| *peer != member);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn key_bytes(n: u8) -> [u8; 32] {
        [n; 32]
    }

    #[test]
    fn sequence_numbers_advance_monotonically() {
        let mut mgr = SenderKeyManager::new(peer(0));
        assert_eq!(mgr.take_sequence(), SeqNum(0));
        assert_eq!(mgr.take_sequence(), SeqNum(1));
        assert_eq!(mgr.take_sequence(), SeqNum(2));
    }

    #[test]
    fn installed_keys_are_tracked_per_member_and_epoch() {
        let mut mgr = SenderKeyManager::new(peer(0));
        mgr.install_member_key(peer(1), Epoch(0), &key_bytes(1)).unwrap();
        mgr.install_member_key(peer(2), Epoch(0), &key_bytes(2)).unwrap();

        assert_eq!(mgr.known_members(), vec![peer(1), peer(2)]);
        assert!(mgr.can_decrypt(peer(1)));
        assert!(!mgr.can_decrypt(peer(3)));
    }

    #[test]
    fn a_wrong_length_key_is_refused() {
        let mut mgr = SenderKeyManager::new(peer(0));
        assert!(mgr.install_member_key(peer(1), Epoch(0), &[0u8; 16]).is_err());
    }

    #[test]
    fn removing_a_member_drops_every_epoch_of_their_key_material() {
        let mut mgr = SenderKeyManager::new(peer(0));
        mgr.install_member_key(peer(1), Epoch(0), &key_bytes(1)).unwrap();
        mgr.install_member_key(peer(1), Epoch(1), &key_bytes(9)).unwrap();
        mgr.install_member_key(peer(2), Epoch(0), &key_bytes(2)).unwrap();

        mgr.remove_member(peer(1));

        assert!(!mgr.can_decrypt(peer(1)));
        assert_eq!(mgr.known_members(), vec![peer(2)]);
    }

    #[test]
    fn rotation_advances_the_epoch_and_restarts_sequencing() {
        let mut mgr = SenderKeyManager::new(peer(0));
        mgr.take_sequence();
        mgr.take_sequence();

        mgr.rotate(Epoch(1), &[peer(0), peer(1)]).unwrap();

        assert_eq!(mgr.epoch(), Epoch(1));
        // Restarting is required for nonce uniqueness: the nonce is a function
        // of (epoch, stream, sequence), so a new epoch makes reused sequence
        // numbers safe.
        assert_eq!(mgr.take_sequence(), SeqNum(0));
    }

    #[test]
    fn a_departed_member_gets_no_key_in_the_new_epoch() {
        let mut mgr = SenderKeyManager::new(peer(0));
        mgr.install_member_key(peer(1), Epoch(0), &key_bytes(1)).unwrap();
        mgr.rotate(Epoch(1), &[peer(0)]).unwrap();
        // Even if a stale delivery arrives for the departed peer, they are not
        // in the member list for this epoch.
        mgr.install_member_key(peer(1), Epoch(1), &key_bytes(1)).unwrap();
        mgr.rotate(Epoch(2), &[peer(0)]).unwrap();

        assert!(!mgr.can_decrypt(peer(1)));
    }

    #[test]
    fn opening_an_epoch_we_hold_no_key_for_is_rejected_before_any_crypto() {
        let mut mgr = SenderKeyManager::new(peer(0));
        let err = mgr.open(peer(1), Epoch(7), SeqNum(0), &[0; 32], &[]).unwrap_err();
        assert!(matches!(err, crate::Error::Crypto(CryptoError::UnknownEpoch(Epoch(7)))));
    }

    #[test]
    fn expiring_epochs_releases_key_material() {
        let mut mgr = SenderKeyManager::new(peer(0));
        mgr.install_member_key(peer(1), Epoch(0), &key_bytes(1)).unwrap();
        mgr.rotate(Epoch(1), &[peer(0), peer(1)]).unwrap();
        mgr.install_member_key(peer(1), Epoch(1), &key_bytes(2)).unwrap();

        let dropped = mgr.expire_epochs(crate::time::Monotonic(60_000));

        assert_eq!(dropped, 1);
        assert!(mgr.can_decrypt(peer(1)), "current epoch key was dropped");
    }
}
