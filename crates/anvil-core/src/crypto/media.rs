//! Media frame encryption: nonces and replay rejection (§48, §81).
//!
//! The two things that are easy to get catastrophically wrong in AEAD media
//! encryption are nonce reuse and replay, so both are handled here explicitly
//! rather than left to the call site.
//!
//! ## Nonce construction
//!
//! ChaCha20-Poly1305 takes a 96-bit nonce, and **reusing one under the same key
//! is fatal** — it leaks the XOR of two plaintexts and, worse, allows forging.
//! Anvil constructs it deterministically instead of randomly:
//!
//! ```text
//!   nonce = salt(12 bytes, per-key, from HKDF)  XOR  (epoch:4 ‖ stream:2 ‖ seq:4 ‖ 0:2)
//! ```
//!
//! Uniqueness follows from the sequence number being unique per (key, stream)
//! — the sender advances it monotonically and the key changes on every epoch.
//! Random nonces would risk collision after ~2^48 frames; derived nonces make
//! it structurally impossible, and the receiver can reconstruct the nonce from
//! the header rather than carrying it on the wire, which saves 12 bytes per
//! packet on a link where every byte is airtime.
//!
//! ## Replay
//!
//! An attacker who captures a packet can resend it (§99). Without a check the
//! receiver would decrypt it happily — the tag is valid, it is a real packet —
//! and the user would hear a word twice, or a "yes" from ten minutes ago.
//!
//! [`ReplayWindow`] is the standard IPSec/DTLS sliding bitmap: accept anything
//! newer than the highest seen, accept an older packet only if it falls inside
//! the window and has not already been seen, reject everything else. A window
//! is necessary rather than a simple counter because real networks reorder, and
//! rejecting all out-of-order packets would discard audio the jitter buffer
//! could have used.

use crate::{CryptoError, Epoch, Result, SeqNum};

/// AEAD nonce length for ChaCha20-Poly1305.
pub const NONCE_LEN: usize = 12;

/// AEAD key length.
pub const KEY_LEN: usize = 32;

/// Replay window width, in packets.
///
/// 64 packets is ~1.3 seconds at 20 ms frames — comfortably wider than any
/// reordering a local network produces, and far narrower than the jitter
/// buffer's tolerance, so a packet old enough to be rejected here was too old
/// to play anyway.
pub const REPLAY_WINDOW: u32 = 64;

/// One epoch's media key for one sender.
///
/// Zeroizes on drop and never prints its contents.
#[derive(Clone, zeroize::ZeroizeOnDrop)]
pub struct MediaKey {
    key: [u8; KEY_LEN],
    salt: [u8; NONCE_LEN],
}

impl MediaKey {
    /// Wrap raw key material.
    #[must_use]
    pub fn new(key: [u8; KEY_LEN], salt: [u8; NONCE_LEN]) -> Self {
        Self { key, salt }
    }

    /// Raw key bytes, for handing to the AEAD.
    #[must_use]
    pub fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.key
    }

    /// Export key plus nonce salt for delivery over an authenticated session.
    #[must_use]
    pub fn material(&self) -> ([u8; KEY_LEN], [u8; NONCE_LEN]) {
        (self.key, self.salt)
    }

    /// Nonce for one frame.
    ///
    /// Deterministic in (epoch, stream, sequence), so sender and receiver
    /// derive the same value without transmitting it.
    #[must_use]
    pub fn nonce(&self, epoch: Epoch, stream: u16, sequence: SeqNum) -> [u8; NONCE_LEN] {
        let mut iv = [0u8; NONCE_LEN];
        iv[0..4].copy_from_slice(&(epoch.0 as u32).to_be_bytes());
        iv[4..6].copy_from_slice(&stream.to_be_bytes());
        iv[6..10].copy_from_slice(&sequence.0.to_be_bytes());

        let mut nonce = self.salt;
        for (n, i) in nonce.iter_mut().zip(iv.iter()) {
            *n ^= *i;
        }
        nonce
    }
}

impl core::fmt::Debug for MediaKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Never print key material (§92).
        f.write_str("MediaKey(redacted)")
    }
}

/// Sliding window of accepted sequence numbers, per (sender, stream, epoch).
#[derive(Clone, Debug)]
pub struct ReplayWindow {
    highest: Option<SeqNum>,
    /// Bit `n` set means "highest − (n+1) has been seen".
    seen: u64,
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayWindow {
    /// A window that has seen nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { highest: None, seen: 0 }
    }

    /// Test and record a sequence number.
    ///
    /// Returns `Ok(())` if the packet is fresh, or
    /// [`CryptoError::ReplayRejected`] if it is a duplicate or too old.
    ///
    /// The window is only updated on acceptance. Recording a packet before it
    /// authenticates would let an attacker poison the window with forged
    /// sequence numbers and censor the real ones — so callers must verify the
    /// AEAD tag *first*, then call this.
    pub fn check_and_update(&mut self, sequence: SeqNum) -> Result<()> {
        let Some(highest) = self.highest else {
            self.highest = Some(sequence);
            return Ok(());
        };

        if sequence == highest {
            return Err(CryptoError::ReplayRejected.into());
        }

        if let Some(advance) = sequence.distance_from(highest) {
            // Newer. Shift the window forward.
            self.seen =
                if advance >= 64 { 0 } else { (self.seen << advance) | (1u64 << (advance - 1)) };
            self.highest = Some(sequence);
            return Ok(());
        }

        // Older: inside the window and unseen, or rejected.
        let age = highest.0.wrapping_sub(sequence.0);
        if age > REPLAY_WINDOW {
            return Err(CryptoError::ReplayRejected.into());
        }

        let bit = 1u64 << (age - 1);
        if self.seen & bit != 0 {
            return Err(CryptoError::ReplayRejected.into());
        }
        self.seen |= bit;
        Ok(())
    }

    /// Highest sequence accepted so far.
    #[must_use]
    pub const fn highest(&self) -> Option<SeqNum> {
        self.highest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> MediaKey {
        MediaKey::new([7u8; KEY_LEN], [3u8; NONCE_LEN])
    }

    #[test]
    fn nonces_are_unique_across_sequence_stream_and_epoch() {
        let k = key();
        let mut seen = std::collections::HashSet::new();

        for epoch in 0..4u64 {
            for stream in 0..4u16 {
                for seq in 0..256u32 {
                    let nonce = k.nonce(Epoch(epoch), stream, SeqNum(seq));
                    assert!(seen.insert(nonce), "nonce reuse at {epoch}/{stream}/{seq}");
                }
            }
        }
    }

    #[test]
    fn nonce_derivation_is_deterministic() {
        let k = key();
        assert_eq!(
            k.nonce(Epoch(1), 0, SeqNum(5)),
            k.nonce(Epoch(1), 0, SeqNum(5)),
            "sender and receiver would derive different nonces"
        );
    }

    #[test]
    fn media_keys_never_print_their_contents() {
        assert_eq!(format!("{:?}", key()), "MediaKey(redacted)");
    }

    #[test]
    fn accepts_an_in_order_stream() {
        let mut w = ReplayWindow::new();
        for seq in 0..1_000 {
            assert!(w.check_and_update(SeqNum(seq)).is_ok(), "rejected in-order {seq}");
        }
    }

    #[test]
    fn rejects_an_exact_replay() {
        let mut w = ReplayWindow::new();
        w.check_and_update(SeqNum(10)).unwrap();
        w.check_and_update(SeqNum(11)).unwrap();

        assert!(w.check_and_update(SeqNum(11)).is_err(), "accepted a replayed packet");
        assert!(w.check_and_update(SeqNum(10)).is_err(), "accepted a replayed packet");
    }

    #[test]
    fn accepts_reordering_inside_the_window() {
        // Real networks reorder; rejecting this would throw away usable audio.
        let mut w = ReplayWindow::new();
        w.check_and_update(SeqNum(100)).unwrap();
        w.check_and_update(SeqNum(103)).unwrap();

        assert!(w.check_and_update(SeqNum(101)).is_ok());
        assert!(w.check_and_update(SeqNum(102)).is_ok());
        // ...but each only once.
        assert!(w.check_and_update(SeqNum(101)).is_err());
    }

    #[test]
    fn rejects_packets_older_than_the_window() {
        let mut w = ReplayWindow::new();
        w.check_and_update(SeqNum(1_000)).unwrap();

        assert!(w.check_and_update(SeqNum(1_000 - REPLAY_WINDOW - 1)).is_err());
        assert!(w.check_and_update(SeqNum(1)).is_err());
    }

    #[test]
    fn a_large_jump_forward_clears_the_window() {
        // After a path switch the sequence can leap; everything before the
        // jump is unreachable and the window must not claim otherwise.
        let mut w = ReplayWindow::new();
        w.check_and_update(SeqNum(10)).unwrap();
        w.check_and_update(SeqNum(10_000)).unwrap();

        assert_eq!(w.highest(), Some(SeqNum(10_000)));
        assert!(w.check_and_update(SeqNum(9_999)).is_ok());
        assert!(w.check_and_update(SeqNum(10)).is_err());
    }

    #[test]
    fn survives_sequence_wrap() {
        let mut w = ReplayWindow::new();
        let start = u32::MAX - 5;

        for i in 0..20u32 {
            let seq = SeqNum(start.wrapping_add(i));
            assert!(w.check_and_update(seq).is_ok(), "rejected {} across wrap", seq.0);
        }
        // A packet from before the wrap is still a replay.
        assert!(w.check_and_update(SeqNum(start)).is_err());
    }

    #[test]
    fn replayed_packets_are_rejected_after_a_long_run() {
        let mut w = ReplayWindow::new();
        for seq in 0..500 {
            w.check_and_update(SeqNum(seq)).unwrap();
        }
        // Captured earlier, replayed now (§99).
        assert!(w.check_and_update(SeqNum(250)).is_err());
        assert!(w.check_and_update(SeqNum(499)).is_err());
    }
}
