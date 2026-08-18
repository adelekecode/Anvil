//! Packet types (§56).

use crate::ProtocolError;

/// What a packet is.
///
/// Discriminants are fixed for the lifetime of protocol v1 — changing one is a
/// wire break, so they are written out explicitly rather than left to the
/// compiler's ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum PacketType {
    // --- session establishment (reliable stream) -------------------------
    /// First contact on a new path.
    Hello = 0x01,
    /// Identity presentation and proof (§45, §67).
    Identity = 0x02,

    // --- room lifecycle (reliable stream) --------------------------------
    /// Announce a newly created room.
    RoomCreate = 0x10,
    /// Ask to join.
    RoomJoin = 0x11,
    /// Admission granted, carrying current room state.
    RoomAccept = 0x12,
    /// Departure, voluntary or announced on someone's behalf.
    RoomLeave = 0x13,
    /// Membership delta.
    Membership = 0x14,

    // --- keys (reliable stream) ------------------------------------------
    /// Deliver sender key material to an authorised member (§49).
    KeyExchange = 0x20,
    /// Advance the epoch after a membership change (§50).
    KeyRotate = 0x21,

    // --- relay (reliable stream) -----------------------------------------
    /// Advertise relay suitability score (§37).
    RelayAnnounce = 0x30,
    /// Election round.
    RelayElection = 0x31,
    /// Committed result; everyone re-points media.
    RelaySwitch = 0x32,

    // --- path management (datagram) --------------------------------------
    /// Probe for RTT on an otherwise idle path.
    PathProbe = 0x40,
    /// Probe reply carrying the sender's view of the path.
    PathReport = 0x41,
    /// Liveness on an idle path. Necessary because VAD means silence produces
    /// no media, and a silent participant must not look like a dead one.
    Heartbeat = 0x42,

    // --- media (datagram) -------------------------------------------------
    /// An encrypted Opus frame.
    Media = 0x50,

    // --- errors ------------------------------------------------------------
    /// Something went wrong; carries a reason.
    Error = 0xF0,
}

impl PacketType {
    /// Whether this type requires reliable ordered delivery (§25).
    ///
    /// The split is the whole reason Anvil uses QUIC rather than raw UDP:
    /// control traffic must arrive, media must arrive *soon* or not at all.
    #[must_use]
    pub const fn is_reliable(self) -> bool {
        !matches!(self, Self::PathProbe | Self::PathReport | Self::Heartbeat | Self::Media)
    }

    /// Whether a relay may forward this packet on a member's behalf.
    ///
    /// Media and liveness, yes. Anything that changes membership, keys or the
    /// relay itself, no — a relay forwarding those would be a relay with a vote,
    /// which is exactly the trust escalation §33 forbids.
    #[must_use]
    pub const fn is_relayable(self) -> bool {
        matches!(self, Self::Media | Self::Heartbeat)
    }
}

impl TryFrom<u8> for PacketType {
    type Error = crate::Error;

    fn try_from(byte: u8) -> Result<Self, crate::Error> {
        Ok(match byte {
            0x01 => Self::Hello,
            0x02 => Self::Identity,
            0x10 => Self::RoomCreate,
            0x11 => Self::RoomJoin,
            0x12 => Self::RoomAccept,
            0x13 => Self::RoomLeave,
            0x14 => Self::Membership,
            0x20 => Self::KeyExchange,
            0x21 => Self::KeyRotate,
            0x30 => Self::RelayAnnounce,
            0x31 => Self::RelayElection,
            0x32 => Self::RelaySwitch,
            0x40 => Self::PathProbe,
            0x41 => Self::PathReport,
            0x42 => Self::Heartbeat,
            0x50 => Self::Media,
            0xF0 => Self::Error,
            other => return Err(ProtocolError::UnknownPacketType(other).into()),
        })
    }
}

impl From<PacketType> for u8 {
    fn from(t: PacketType) -> u8 {
        t as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: &[PacketType] = &[
        PacketType::Hello,
        PacketType::Identity,
        PacketType::RoomCreate,
        PacketType::RoomJoin,
        PacketType::RoomAccept,
        PacketType::RoomLeave,
        PacketType::Membership,
        PacketType::KeyExchange,
        PacketType::KeyRotate,
        PacketType::RelayAnnounce,
        PacketType::RelayElection,
        PacketType::RelaySwitch,
        PacketType::PathProbe,
        PacketType::PathReport,
        PacketType::Heartbeat,
        PacketType::Media,
        PacketType::Error,
    ];

    #[test]
    fn every_type_round_trips_through_its_byte() {
        for &t in ALL {
            assert_eq!(PacketType::try_from(u8::from(t)).unwrap(), t);
        }
    }

    #[test]
    fn unknown_bytes_are_rejected() {
        assert!(PacketType::try_from(0x00).is_err());
        assert!(PacketType::try_from(0x99).is_err());
    }

    #[test]
    fn media_and_liveness_are_unreliable_everything_else_is_not() {
        assert!(!PacketType::Media.is_reliable());
        assert!(!PacketType::Heartbeat.is_reliable());
        assert!(PacketType::RoomJoin.is_reliable());
        assert!(PacketType::KeyExchange.is_reliable());
        assert!(PacketType::RelaySwitch.is_reliable());
    }

    #[test]
    fn a_relay_may_not_forward_anything_that_confers_authority() {
        for &t in ALL {
            if t.is_relayable() {
                assert!(
                    matches!(t, PacketType::Media | PacketType::Heartbeat),
                    "{t:?} became relayable; a relay must not carry control traffic"
                );
            }
        }
        assert!(!PacketType::KeyExchange.is_relayable());
        assert!(!PacketType::Membership.is_relayable());
        assert!(!PacketType::RelaySwitch.is_relayable());
    }
}
