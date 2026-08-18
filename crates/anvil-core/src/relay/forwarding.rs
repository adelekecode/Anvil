//! Encrypted fan-out (§33, §74).
//!
//! What a relay does, in full:
//!
//! ```text
//!   receive packet
//!     ─► check it is a type a relay may forward
//!     ─► check it has not already been relayed
//!     ─► read the header (routing only — the payload stays sealed)
//!     ─► mark it relayed
//!     ─► send to every member except the sender
//! ```
//!
//! What a relay does **not** do: decrypt, decode, mix, re-encode, re-encrypt,
//! inspect payloads, or make membership decisions. It has no media keys, and
//! being elected does not give it any (§33). Every one of those omissions is a
//! deliberate architectural choice, and this module is where they are enforced
//! rather than merely intended.
//!
//! ## Fan-out is the point
//!
//! Alice uploads one copy; the relay sends N−1. Compared with full mesh, Alice's
//! upload drops from N−1 streams to one — the difference between a phone
//! comfortably handling a four-person room and a phone with three simultaneous
//! encoded uplinks over one radio.
//!
//! ## Loop prevention
//!
//! During a relay change two nodes can briefly each believe they are the relay.
//! Without a guard they forward to each other forever, at line rate, until the
//! battery dies. [`crate::protocol::FLAG_RELAYED`] makes this a one-line
//! problem: a packet is forwarded at most once, and the flag is excluded from
//! the AEAD associated data precisely so a relay can set it without holding a
//! key.

use crate::protocol::MediaHeader;
use crate::PeerId;

/// What to do with a packet that arrived at a relay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ForwardDecision {
    /// Send to these members.
    Forward {
        /// Recipients — everyone except the sender.
        to: Vec<PeerId>,
    },
    /// Drop it, with a reason for diagnostics.
    Drop {
        /// Why.
        reason: DropReason,
    },
}

/// Why a relay refused to forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropReason {
    /// Not a type a relay may carry — control traffic never passes through
    /// a relay (§33).
    NotRelayable,
    /// Already forwarded once. Loop guard.
    AlreadyRelayed,
    /// Sender is not a member of this room.
    UnknownSender,
    /// Nobody to send it to.
    NoRecipients,
}

/// Decide what a relay should do with a packet.
///
/// Pure and side-effect free, so the forwarding rules can be tested
/// exhaustively — including the ones that exist to stop a malicious relay from
/// being useful, which are exactly the ones nobody exercises by hand.
#[must_use]
pub fn decide(header: &MediaHeader, members: &[PeerId], sender: Option<PeerId>) -> ForwardDecision {
    if !header.packet_type.is_relayable() {
        return ForwardDecision::Drop { reason: DropReason::NotRelayable };
    }

    if header.is_relayed() {
        return ForwardDecision::Drop { reason: DropReason::AlreadyRelayed };
    }

    let Some(sender) = sender else {
        return ForwardDecision::Drop { reason: DropReason::UnknownSender };
    };

    if !members.contains(&sender) {
        return ForwardDecision::Drop { reason: DropReason::UnknownSender };
    }

    let to: Vec<PeerId> = members.iter().copied().filter(|m| *m != sender).collect();
    if to.is_empty() {
        return ForwardDecision::Drop { reason: DropReason::NoRecipients };
    }

    ForwardDecision::Forward { to }
}

/// Resolve a header's truncated sender route id to a member.
///
/// Truncation means collisions are possible, though vanishingly unlikely among
/// the handful of members in a room. A collision is resolved by returning
/// `None` rather than by guessing — forwarding a packet attributed to the wrong
/// sender would corrupt the receiver's replay state for both of them.
#[must_use]
pub fn resolve_sender(route_id: u32, members: &[PeerId]) -> Option<PeerId> {
    let mut matches = members.iter().filter(|m| m.route_id() == route_id);
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(*first)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketType;
    use crate::{Epoch, MediaTimestamp, SeqNum};

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    fn header(kind: PacketType) -> MediaHeader {
        MediaHeader::new(kind, 1, peer(1).route_id(), 0, SeqNum(1), MediaTimestamp(0), Epoch(0))
    }

    #[test]
    fn media_fans_out_to_everyone_but_the_sender() {
        // §74: Alice uploads once, relay sends to Bob, Chris and David.
        let members = [peer(1), peer(2), peer(3), peer(4)];
        let decision = decide(&header(PacketType::Media), &members, Some(peer(1)));

        match decision {
            ForwardDecision::Forward { to } => {
                assert_eq!(to, vec![peer(2), peer(3), peer(4)]);
                assert!(!to.contains(&peer(1)), "echoed the packet back to its sender");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_relay_refuses_every_control_message() {
        // The privilege boundary: a relay that could forward these would have a
        // hand in membership, keys and its own succession.
        let members = [peer(1), peer(2)];
        for kind in [
            PacketType::KeyExchange,
            PacketType::KeyRotate,
            PacketType::Membership,
            PacketType::RoomJoin,
            PacketType::RelaySwitch,
            PacketType::Identity,
        ] {
            assert_eq!(
                decide(&header(kind), &members, Some(peer(1))),
                ForwardDecision::Drop { reason: DropReason::NotRelayable },
                "{kind:?} was forwardable"
            );
        }
    }

    #[test]
    fn a_packet_is_never_forwarded_twice() {
        let members = [peer(1), peer(2), peer(3)];
        let mut h = header(PacketType::Media);
        h.mark_relayed();

        assert_eq!(
            decide(&h, &members, Some(peer(1))),
            ForwardDecision::Drop { reason: DropReason::AlreadyRelayed }
        );
    }

    #[test]
    fn packets_from_non_members_are_dropped() {
        let members = [peer(1), peer(2)];
        assert_eq!(
            decide(&header(PacketType::Media), &members, Some(peer(9))),
            ForwardDecision::Drop { reason: DropReason::UnknownSender }
        );
        assert_eq!(
            decide(&header(PacketType::Media), &members, None),
            ForwardDecision::Drop { reason: DropReason::UnknownSender }
        );
    }

    #[test]
    fn a_lone_member_produces_no_fan_out() {
        let members = [peer(1)];
        assert_eq!(
            decide(&header(PacketType::Media), &members, Some(peer(1))),
            ForwardDecision::Drop { reason: DropReason::NoRecipients }
        );
    }

    #[test]
    fn heartbeats_are_relayed_so_silent_peers_stay_visible() {
        // VAD means a quiet participant sends no media; without relayed
        // heartbeats they would look dead.
        let members = [peer(1), peer(2)];
        assert!(matches!(
            decide(&header(PacketType::Heartbeat), &members, Some(peer(1))),
            ForwardDecision::Forward { .. }
        ));
    }

    #[test]
    fn sender_resolution_refuses_to_guess_on_collision() {
        let members = [peer(1), peer(2)];
        assert_eq!(resolve_sender(peer(1).route_id(), &members), Some(peer(1)));
        assert_eq!(resolve_sender(0xdead_beef, &members), None);

        // Two members whose route ids collide: return None rather than picking.
        let mut twin = [0u8; 32];
        twin[..4].copy_from_slice(&peer(1).0[..4]);
        twin[31] = 99;
        let colliding = [peer(1), PeerId(twin)];
        assert_eq!(resolve_sender(peer(1).route_id(), &colliding), None);
    }
}
