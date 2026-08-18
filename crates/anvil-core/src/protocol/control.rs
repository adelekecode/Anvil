//! Control messages (§25, §56).
//!
//! Control traffic travels on QUIC reliable streams, never as datagrams. It is
//! low-volume, it must arrive, and order matters: a `KeyRotate` overtaking the
//! `Membership` change that caused it would leave receivers holding keys for a
//! room state they have not been told about.
//!
//! **Control messages are never relayed.** [`PacketType::is_relayable`] is
//! false for every type here, and that is enforced by test. A relay that could
//! forward a `Membership` or `KeyExchange` message would be a relay with a say
//! in who is in the room and who holds keys — which is precisely the privilege
//! escalation §33 exists to prevent. Control flows peer-to-peer over
//! authenticated sessions.
//!
//! ## Phase status
//!
//! The message set below is the v0.1 vocabulary. Binary encoding is deliberately
//! not fixed here yet — it should be pinned in `protocol/packet-format.md` and
//! implemented alongside the Phase 1 QUIC work, when the exact fields each
//! message needs are known from working code rather than guessed from a
//! diagram. Guessing an encoding first and discovering the field list second is
//! how wire formats acquire reserved bytes that never get used.

use super::PacketType;
use crate::{Epoch, PeerId, RoomId};

/// A control message.
///
/// Payloads carry the minimum each recipient needs to act. Notably, none of
/// them carries a participant's media key — key delivery is per-recipient over
/// an authenticated session (§49), never broadcast.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum ControlMessage {
    /// Opening message on a fresh path. Carries the protocol version so a
    /// mismatch is caught before anything else is attempted.
    Hello {
        /// Sender's wire version.
        version: u8,
        /// Truncated identity fingerprint, to correlate with discovery.
        fingerprint: crate::discovery::Fingerprint,
    },

    /// Identity presentation: full public identity key plus a signature over a
    /// session-binding challenge, proving possession of the private half and
    /// tying it to *this* connection rather than a replayed one.
    Identity {
        /// Ed25519 public identity key.
        public_key: [u8; 32],
        /// Ephemeral X25519 public key for session key agreement (§47).
        ephemeral_key: [u8; 32],
        /// Signature over the transcript.
        signature: [u8; 64],
        /// Self-asserted display name. Authenticated as *sent by this key*,
        /// which is not the same as true.
        display_name: String,
    },

    /// Request admission to a room.
    RoomJoin {
        /// Which room.
        room: RoomId,
        /// Admission credential, when the room uses a join code (§68).
        credential: Option<Vec<u8>>,
    },

    /// Admission granted, with everything needed to participate.
    RoomAccept {
        /// Which room.
        room: RoomId,
        /// Current key generation.
        epoch: Epoch,
        /// Current members.
        members: Vec<PeerId>,
        /// Current relay, if one has been elected.
        relay: Option<PeerId>,
    },

    /// Someone left, or is being announced as gone.
    RoomLeave {
        /// Who.
        peer: PeerId,
        /// Why, for the UI.
        reason: String,
    },

    /// Membership changed. Always paired with an epoch advance.
    Membership {
        /// New epoch this change produces.
        epoch: Epoch,
        /// Members added.
        added: Vec<PeerId>,
        /// Members removed.
        removed: Vec<PeerId>,
    },

    /// Deliver the sender's media key for an epoch to one authorised member.
    ///
    /// Sent once per recipient over that recipient's authenticated session. It
    /// is not fan-out traffic and must never pass through a relay.
    KeyExchange {
        /// Epoch this key belongs to.
        epoch: Epoch,
        /// Key material, already encrypted to the recipient's session.
        sealed_key: Vec<u8>,
    },

    /// Advance to a new epoch. Old keys are discarded on receipt, which is what
    /// makes departure actually mean something (§50).
    KeyRotate {
        /// The new epoch.
        epoch: Epoch,
    },

    /// Advertise relay suitability (§37, §38).
    RelayAnnounce {
        /// Self-reported score.
        ///
        /// Self-reported, and therefore *lying is possible*: a device can claim
        /// a perfect score to capture the relay role. That buys it the ability
        /// to drop and delay packets, which it could largely do anyway as a
        /// participant, and buys it no cryptographic access whatsoever. Worth
        /// stating explicitly in `protocol/relay-election.md` rather than
        /// leaving as an unexamined assumption.
        score: f32,
        /// Term this announcement is for, to reject stale announcements.
        term: u64,
    },

    /// Election round.
    RelayElection {
        /// Election term.
        term: u64,
        /// Who the sender believes should relay.
        candidate: PeerId,
    },

    /// Committed election result; everyone re-points media.
    RelaySwitch {
        /// Term this result belongs to.
        term: u64,
        /// The new relay.
        relay: PeerId,
    },

    /// Something went wrong.
    Error {
        /// Machine-readable code.
        code: u16,
        /// Human-readable detail.
        detail: String,
    },
}

impl ControlMessage {
    /// The packet type byte for this message.
    #[must_use]
    pub const fn packet_type(&self) -> PacketType {
        match self {
            Self::Hello { .. } => PacketType::Hello,
            Self::Identity { .. } => PacketType::Identity,
            Self::RoomJoin { .. } => PacketType::RoomJoin,
            Self::RoomAccept { .. } => PacketType::RoomAccept,
            Self::RoomLeave { .. } => PacketType::RoomLeave,
            Self::Membership { .. } => PacketType::Membership,
            Self::KeyExchange { .. } => PacketType::KeyExchange,
            Self::KeyRotate { .. } => PacketType::KeyRotate,
            Self::RelayAnnounce { .. } => PacketType::RelayAnnounce,
            Self::RelayElection { .. } => PacketType::RelayElection,
            Self::RelaySwitch { .. } => PacketType::RelaySwitch,
            Self::Error { .. } => PacketType::Error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_control_message_is_ever_relayable() {
        let messages = [
            ControlMessage::Hello { version: 1, fingerprint: [0; 8] },
            ControlMessage::RoomJoin { room: RoomId::generate(), credential: None },
            ControlMessage::Membership { epoch: Epoch(1), added: vec![], removed: vec![] },
            ControlMessage::KeyRotate { epoch: Epoch(2) },
            ControlMessage::RelaySwitch { term: 1, relay: PeerId::UNSPECIFIED },
            ControlMessage::Error { code: 1, detail: String::new() },
        ];

        for message in messages {
            assert!(
                !message.packet_type().is_relayable(),
                "{:?} would be forwardable by a relay",
                message.packet_type()
            );
            assert!(message.packet_type().is_reliable());
        }
    }
}
