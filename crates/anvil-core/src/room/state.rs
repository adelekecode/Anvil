//! Room state (§69, §70).
//!
//! There is no database and no server. Every participant keeps its own copy of
//! room state and they converge through authenticated control messages. That is
//! a distributed system, with all the usual consequences, and pretending
//! otherwise is how "everyone sees a different participant list" happens.
//!
//! Two rules keep it tractable at v0.1 scale:
//!
//! * **Membership changes carry an epoch**, and an epoch only ever moves
//!   forward. A message describing an older epoch is stale and is discarded, so
//!   reordered control traffic cannot resurrect a departed member.
//! * **The relay is not the authority** (§71). It may help distribute topology,
//!   but the room outlives it, so no state here is keyed on who is relaying.

use std::collections::BTreeMap;

use crate::time::Monotonic;
use crate::{Epoch, PeerId, RoomError, RoomId, Result};

use super::membership::Participant;

/// This node's view of a room.
#[derive(Clone, Debug)]
pub struct RoomState {
    /// Room identity.
    pub room_id: RoomId,
    /// This device.
    pub local_peer_id: PeerId,
    /// Current key generation.
    pub epoch: Epoch,
    /// Members, ordered by id so every participant iterates identically —
    /// which matters for deterministic relay election tie-breaks.
    pub participants: BTreeMap<PeerId, Participant>,
    /// Current relay, if elected. `None` during an election, and in a
    /// two-person room, where media goes direct (§35).
    pub relay: Option<PeerId>,
    /// Whether this node created the room and handles admission (§68).
    pub is_host: bool,
    /// When the room was created or joined.
    pub since: Monotonic,
}

impl RoomState {
    /// Create a room hosted by this device.
    #[must_use]
    pub fn create(local_peer_id: PeerId, display_name: String, now: Monotonic) -> Self {
        let mut participants = BTreeMap::new();
        participants.insert(local_peer_id, Participant::new(local_peer_id, display_name, now));

        Self {
            room_id: RoomId::generate(),
            local_peer_id,
            epoch: Epoch(0),
            participants,
            relay: None,
            is_host: true,
            since: now,
        }
    }

    /// Adopt state received in a `RoomAccept`.
    #[must_use]
    pub fn joined(
        room_id: RoomId,
        local_peer_id: PeerId,
        epoch: Epoch,
        participants: Vec<Participant>,
        relay: Option<PeerId>,
        now: Monotonic,
    ) -> Self {
        Self {
            room_id,
            local_peer_id,
            epoch,
            participants: participants.into_iter().map(|p| (p.peer_id, p)).collect(),
            relay,
            is_host: false,
            since: now,
        }
    }

    /// Number of participants, including this device.
    #[must_use]
    pub fn size(&self) -> usize {
        self.participants.len()
    }

    /// Whether media should go peer-to-peer rather than through a relay.
    ///
    /// Two people need no relay (§35): adding one would insert a hop, latency
    /// and a battery cost for no fan-out benefit whatsoever.
    #[must_use]
    pub fn is_direct(&self) -> bool {
        self.size() <= 2
    }

    /// Whether this device is currently the relay.
    #[must_use]
    pub fn is_relay(&self) -> bool {
        self.relay == Some(self.local_peer_id)
    }

    /// Everyone except this device.
    pub fn others(&self) -> impl Iterator<Item = &Participant> {
        self.participants.values().filter(|p| p.peer_id != self.local_peer_id)
    }

    /// Look up a participant.
    #[must_use]
    pub fn participant(&self, peer_id: PeerId) -> Option<&Participant> {
        self.participants.get(&peer_id)
    }

    /// Add a participant and advance the epoch (§50).
    ///
    /// Rejects an epoch that does not move forward, which is how stale or
    /// replayed membership messages are prevented from mutating the room.
    pub fn add_participant(&mut self, participant: Participant, epoch: Epoch) -> Result<()> {
        if epoch <= self.epoch {
            return Err(RoomError::JoinRejected(format!(
                "stale membership change: {epoch} is not newer than {}",
                self.epoch
            ))
            .into());
        }
        self.participants.insert(participant.peer_id, participant);
        self.epoch = epoch;
        Ok(())
    }

    /// Remove a participant and advance the epoch.
    pub fn remove_participant(&mut self, peer_id: PeerId, epoch: Epoch) -> Result<Participant> {
        if epoch <= self.epoch {
            return Err(RoomError::NotAMember(peer_id).into());
        }
        let participant =
            self.participants.remove(&peer_id).ok_or(RoomError::NotAMember(peer_id))?;
        self.epoch = epoch;

        // Losing the relay does not end the room; it triggers an election (§41).
        if self.relay == Some(peer_id) {
            self.relay = None;
        }
        Ok(participant)
    }

    /// Install the result of a relay election.
    pub fn set_relay(&mut self, relay: Option<PeerId>) -> Result<()> {
        if let Some(peer) = relay {
            if !self.participants.contains_key(&peer) {
                return Err(RoomError::NotAMember(peer).into());
            }
        }
        self.relay = relay;
        Ok(())
    }

    /// An immutable view for the host.
    #[must_use]
    pub fn snapshot(&self) -> RoomSnapshot {
        RoomSnapshot {
            room_id: self.room_id,
            epoch: self.epoch,
            participants: self.participants.values().cloned().collect(),
            relay: self.relay,
            local_peer_id: self.local_peer_id,
            is_host: self.is_host,
            is_direct: self.is_direct(),
        }
    }
}

/// A point-in-time copy of room state, sent to the host in events.
///
/// A separate type from [`RoomState`] so the UI cannot hold a reference into
/// live protocol state — the engine owns that, exclusively.
#[derive(Clone, Debug)]
pub struct RoomSnapshot {
    /// Room identity.
    pub room_id: RoomId,
    /// Key generation.
    pub epoch: Epoch,
    /// Members.
    pub participants: Vec<Participant>,
    /// Current relay.
    pub relay: Option<PeerId>,
    /// This device.
    pub local_peer_id: PeerId,
    /// Whether this device hosts.
    pub is_host: bool,
    /// Whether media is peer-to-peer.
    pub is_direct: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn participant(n: u8) -> Participant {
        Participant::new(peer(n), format!("peer{n}"), Monotonic::ZERO)
    }

    #[test]
    fn a_new_room_contains_only_its_host() {
        let room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);

        assert_eq!(room.size(), 1);
        assert!(room.is_host);
        assert!(room.is_direct());
        assert_eq!(room.relay, None);
        assert_eq!(room.others().count(), 0);
    }

    #[test]
    fn two_people_need_no_relay_three_do() {
        let mut room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        room.add_participant(participant(2), Epoch(1)).unwrap();
        assert!(room.is_direct(), "a two-person room must not insert a relay hop");

        room.add_participant(participant(3), Epoch(2)).unwrap();
        assert!(!room.is_direct());
    }

    #[test]
    fn membership_changes_advance_the_epoch() {
        let mut room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        assert_eq!(room.epoch, Epoch(0));

        room.add_participant(participant(2), Epoch(1)).unwrap();
        assert_eq!(room.epoch, Epoch(1));

        room.remove_participant(peer(2), Epoch(2)).unwrap();
        assert_eq!(room.epoch, Epoch(2));
        assert_eq!(room.size(), 1);
    }

    #[test]
    fn stale_membership_messages_cannot_resurrect_a_departed_member() {
        // The attack: replay the join message for someone who has since left.
        let mut room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        room.add_participant(participant(2), Epoch(1)).unwrap();
        room.remove_participant(peer(2), Epoch(2)).unwrap();

        assert!(room.add_participant(participant(2), Epoch(1)).is_err());
        assert_eq!(room.size(), 1);
        assert!(room.participant(peer(2)).is_none());
    }

    #[test]
    fn losing_the_relay_clears_it_without_ending_the_room() {
        let mut room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        room.add_participant(participant(2), Epoch(1)).unwrap();
        room.add_participant(participant(3), Epoch(2)).unwrap();
        room.set_relay(Some(peer(3))).unwrap();
        assert!(!room.is_relay());

        room.remove_participant(peer(3), Epoch(3)).unwrap();

        assert_eq!(room.relay, None, "relay slot should be vacant, pending election");
        assert_eq!(room.size(), 2, "the room must survive its relay leaving");
    }

    #[test]
    fn a_non_member_cannot_be_made_relay() {
        let mut room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        assert!(room.set_relay(Some(peer(9))).is_err());
    }

    #[test]
    fn removing_someone_who_is_not_a_member_fails() {
        let mut room = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        assert!(room.remove_participant(peer(9), Epoch(1)).is_err());
    }

    #[test]
    fn participant_ordering_is_identical_on_every_device() {
        // Election tie-breaks depend on this.
        let mut a = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        a.add_participant(participant(3), Epoch(1)).unwrap();
        a.add_participant(participant(2), Epoch(2)).unwrap();

        let mut b = RoomState::create(peer(1), "Alice".into(), Monotonic::ZERO);
        b.add_participant(participant(2), Epoch(1)).unwrap();
        b.add_participant(participant(3), Epoch(2)).unwrap();

        let order_a: Vec<PeerId> = a.participants.keys().copied().collect();
        let order_b: Vec<PeerId> = b.participants.keys().copied().collect();
        assert_eq!(order_a, order_b);
    }
}
