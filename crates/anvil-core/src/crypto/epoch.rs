//! Key epochs (§50).
//!
//! Every membership change advances the epoch and produces new key material.
//! That is what makes "David left" mean something: after the rotation David
//! holds keys for a generation nobody is using any more.
//!
//! ## The retention problem
//!
//! Rotation cannot be instantaneous. When David leaves at epoch 41→42, packets
//! encrypted under epoch 41 are still in flight — a couple of hundred
//! milliseconds of them on a jittery path. Discarding epoch 41 the instant the
//! rotation is decided drops audio that was legitimately sent by legitimate
//! members, and the user hears the room glitch on every join and leave.
//!
//! So old keys are retained briefly. The window is a genuine trade-off and it
//! should be set with eyes open:
//!
//! * too short → audible dropouts on every membership change;
//! * too long → a departed member can decrypt for that much longer.
//!
//! [`RETENTION`] is sized to comfortably cover the maximum jitter buffer depth
//! plus a path switch. It is *not* sized for convenience, and it is deliberately
//! shorter than the time it takes a person to walk out of Wi-Fi range.
//!
//! Note this window is a bound on decryption of *already-sent* traffic. A
//! departed member never receives new key material, so they cannot follow the
//! conversation forward under any retention setting.

use core::time::Duration;
use std::collections::HashMap;

use crate::time::Monotonic;
use crate::Epoch;

/// How long key material for a superseded epoch stays usable for decryption.
pub const RETENTION: Duration = Duration::from_millis(500);

/// Tracks the current epoch and which superseded epochs are still acceptable.
#[derive(Debug)]
pub struct EpochManager {
    current: Epoch,
    /// Superseded epoch → when it was superseded.
    retired: HashMap<Epoch, Monotonic>,
}

impl EpochManager {
    /// Start at epoch 0.
    #[must_use]
    pub fn new() -> Self {
        Self { current: Epoch(0), retired: HashMap::new() }
    }

    /// The epoch new traffic is sent under.
    #[must_use]
    pub const fn current(&self) -> Epoch {
        self.current
    }

    /// Advance to a new epoch, retiring the current one.
    ///
    /// Ignores an epoch that is not newer: rotations can arrive out of order
    /// during a membership storm, and going backwards would mean sending under
    /// a key some members have already discarded.
    pub fn advance(&mut self, new_epoch: Epoch, now: Monotonic) -> bool {
        if new_epoch <= self.current {
            return false;
        }
        self.retired.insert(self.current, now);
        self.current = new_epoch;
        true
    }

    /// Whether a packet claiming `epoch` may still be decrypted.
    #[must_use]
    pub fn accepts(&self, epoch: Epoch, now: Monotonic) -> bool {
        if epoch == self.current {
            return true;
        }
        self.retired
            .get(&epoch)
            .is_some_and(|retired_at| now.saturating_since(*retired_at) <= RETENTION)
    }

    /// Drop key material past its retention window.
    ///
    /// Returns the epochs dropped, so the key manager can zeroize them. This
    /// must be called on the engine tick — retention that is only enforced
    /// lazily on the next packet is retention that lasts until the room goes
    /// quiet, which is exactly when it matters most.
    pub fn expire(&mut self, now: Monotonic) -> Vec<Epoch> {
        let expired: Vec<Epoch> = self
            .retired
            .iter()
            .filter(|(_, retired_at)| now.saturating_since(**retired_at) > RETENTION)
            .map(|(epoch, _)| *epoch)
            .collect();

        for epoch in &expired {
            self.retired.remove(epoch);
        }
        expired
    }

    /// How many superseded epochs are still held.
    #[must_use]
    pub fn retained_count(&self) -> usize {
        self.retired.len()
    }
}

impl Default for EpochManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_current_epoch() {
        let mgr = EpochManager::new();
        assert!(mgr.accepts(Epoch(0), Monotonic(1_000)));
        assert!(!mgr.accepts(Epoch(1), Monotonic(1_000)));
    }

    #[test]
    fn in_flight_packets_survive_a_rotation() {
        // Someone joins mid-sentence; audio must not glitch.
        let mut mgr = EpochManager::new();
        assert!(mgr.advance(Epoch(1), Monotonic(10_000)));

        assert!(mgr.accepts(Epoch(1), Monotonic(10_050)));
        assert!(mgr.accepts(Epoch(0), Monotonic(10_050)), "dropped in-flight audio");
    }

    #[test]
    fn a_departed_member_stops_being_able_to_decrypt() {
        let mut mgr = EpochManager::new();
        mgr.advance(Epoch(1), Monotonic(10_000));

        let past_retention = Monotonic(10_000 + RETENTION.as_millis() as u64 + 1);
        assert!(!mgr.accepts(Epoch(0), past_retention));
    }

    #[test]
    fn expiry_actively_releases_key_material() {
        let mut mgr = EpochManager::new();
        mgr.advance(Epoch(1), Monotonic(1_000));
        assert_eq!(mgr.retained_count(), 1);

        assert!(mgr.expire(Monotonic(1_100)).is_empty());
        assert_eq!(mgr.expire(Monotonic(5_000)), vec![Epoch(0)]);
        assert_eq!(mgr.retained_count(), 0);
    }

    #[test]
    fn rotations_never_go_backwards() {
        let mut mgr = EpochManager::new();
        mgr.advance(Epoch(5), Monotonic(1_000));

        assert!(!mgr.advance(Epoch(3), Monotonic(2_000)));
        assert!(!mgr.advance(Epoch(5), Monotonic(2_000)));
        assert_eq!(mgr.current(), Epoch(5));
    }

    #[test]
    fn rapid_rotations_retain_each_generation_independently() {
        // Three people join in quick succession.
        let mut mgr = EpochManager::new();
        mgr.advance(Epoch(1), Monotonic(1_000));
        mgr.advance(Epoch(2), Monotonic(1_100));
        mgr.advance(Epoch(3), Monotonic(1_200));

        assert!(mgr.accepts(Epoch(3), Monotonic(1_250)));
        assert!(mgr.accepts(Epoch(2), Monotonic(1_250)));
        assert!(mgr.accepts(Epoch(1), Monotonic(1_250)));

        // Each expires on its own schedule, not all at once.
        assert_eq!(mgr.expire(Monotonic(1_550)).len(), 1);
        assert_eq!(mgr.retained_count(), 2);
    }
}
