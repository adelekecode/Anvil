//! Room topology (§31, §35, §75).
//!
//! Anvil has exactly two shapes, and v0.1 does not invent a third:
//!
//! ```text
//!   Direct (2 people)            Relayed (3+)
//!                                        Bob
//!   Alice ◄────────► Bob                  │
//!                                Alice ── R ── Chris
//!                                         │
//!                                       David
//! ```
//!
//! Arbitrary multi-hop routing is out of scope (§36) and should stay that way
//! for v0.1. Phones make poor routers: the OS backgrounds them, the radio
//! sleeps, and a route through three handsets has three chances to break.
//! Direct connectivity plus one elected relay covers the target rooms without
//! any of that.

use crate::PeerId;

/// How media flows in a room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Topology {
    /// Two participants, media peer-to-peer (§35).
    Direct {
        /// The other participant.
        peer: PeerId,
    },
    /// Three or more, media through the relay (§31).
    Relayed {
        /// The elected relay.
        relay: PeerId,
        /// Whether this device is it.
        is_local: bool,
    },
    /// In a room but with no route — during an election, or while every path
    /// is down. Distinct from "not in a room": the session is alive and
    /// expected to recover.
    Pending,
}

impl Topology {
    /// Work out the topology for a room's current state.
    #[must_use]
    pub fn resolve(local: PeerId, members: &[PeerId], relay: Option<PeerId>) -> Self {
        match members.len() {
            0 | 1 => Self::Pending,
            2 => {
                let peer = members.iter().copied().find(|m| *m != local);
                peer.map_or(Self::Pending, |peer| Self::Direct { peer })
            }
            _ => match relay {
                Some(relay) => Self::Relayed { relay, is_local: relay == local },
                None => Self::Pending,
            },
        }
    }

    /// Where to send outgoing media.
    ///
    /// One destination in every case — that is the whole point of the relay
    /// (§74). A sender never fans out.
    #[must_use]
    pub fn media_destination(&self) -> Option<PeerId> {
        match self {
            Self::Direct { peer } => Some(*peer),
            // A relay still sends its own media through itself conceptually;
            // the engine short-circuits to local fan-out.
            Self::Relayed { relay, is_local } => (!is_local).then_some(*relay),
            Self::Pending => None,
        }
    }

    /// Whether this device must fan packets out to others.
    #[must_use]
    pub const fn is_forwarding(&self) -> bool {
        matches!(self, Self::Relayed { is_local: true, .. })
    }

    /// Whether media can flow at all right now.
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        !matches!(self, Self::Pending)
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

    #[test]
    fn two_people_talk_directly() {
        let topology = Topology::resolve(peer(1), &[peer(1), peer(2)], None);

        assert_eq!(topology, Topology::Direct { peer: peer(2) });
        assert_eq!(topology.media_destination(), Some(peer(2)));
        assert!(!topology.is_forwarding());
    }

    #[test]
    fn a_relay_is_ignored_in_a_two_person_room() {
        // Inserting a hop between two people would be pure cost.
        let topology = Topology::resolve(peer(1), &[peer(1), peer(2)], Some(peer(2)));
        assert_eq!(topology, Topology::Direct { peer: peer(2) });
    }

    #[test]
    fn three_people_route_through_the_relay() {
        let members = [peer(1), peer(2), peer(3)];
        let topology = Topology::resolve(peer(1), &members, Some(peer(2)));

        assert_eq!(topology, Topology::Relayed { relay: peer(2), is_local: false });
        assert_eq!(topology.media_destination(), Some(peer(2)));
        assert!(!topology.is_forwarding());
    }

    #[test]
    fn the_relay_itself_forwards_rather_than_sending_to_itself() {
        let members = [peer(1), peer(2), peer(3)];
        let topology = Topology::resolve(peer(2), &members, Some(peer(2)));

        assert!(topology.is_forwarding());
        assert_eq!(topology.media_destination(), None);
    }

    #[test]
    fn a_group_room_with_no_relay_is_pending_not_broken() {
        // Mid-election. The room is alive; media is briefly stalled.
        let members = [peer(1), peer(2), peer(3)];
        let topology = Topology::resolve(peer(1), &members, None);

        assert_eq!(topology, Topology::Pending);
        assert!(!topology.is_usable());
        assert_eq!(topology.media_destination(), None);
    }

    #[test]
    fn a_room_of_one_is_pending() {
        assert_eq!(Topology::resolve(peer(1), &[peer(1)], None), Topology::Pending);
    }
}
