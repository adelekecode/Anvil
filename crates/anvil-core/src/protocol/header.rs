//! The media packet header (§53, §55).
//!
//! This header is the exact boundary between what a relay may see and what it
//! may not. Every field here is visible to a forwarding node, so every field
//! had to justify itself:
//!
//! | Field | Bytes | Why a relay needs it |
//! |---|---|---|
//! | `version` | 1 | reject incompatible traffic without parsing further |
//! | `packet_type` | 1 | decide whether it is even forwardable |
//! | `flags` | 1 | hop marking, loop prevention |
//! | `room_route_id` | 4 | which room's members to fan out to |
//! | `sender_route_id` | 4 | who not to echo the packet back to |
//! | `stream_id` | 2 | per-stream ordering at the receiver |
//! | `sequence` | 4 | loss detection, replay rejection |
//! | `timestamp` | 4 | jitter buffer playout spacing |
//! | `epoch` | 2 | which key generation the receiver should use |
//!
//! **23 bytes.** With a 16-byte AEAD tag that is 39 bytes of overhead on a
//! ~60-byte Opus frame — roughly 15 kbps per stream at 50 packets/second, on
//! top of a 24 kbps payload. That is a real cost and it is worth stating: on a
//! congested radio with four participants it is the difference between fitting
//! and not.
//!
//! It is accepted for v1 because the alternative — implicit or compressed
//! header state — requires per-path context the relay would have to maintain,
//! and getting that wrong breaks recovery after a relay change. A future
//! version can shrink this with a negotiated compression scheme, which is
//! exactly why [`crate::PROTOCOL_VERSION`] is the first byte on the wire.
//!
//! Note also what is *absent*: no full peer id, no room id, no display name, no
//! participant list. A relay learns that a packet exists, roughly how big it is,
//! and when — the metadata exposure §54 acknowledges — but not who is in the
//! room or what was said.

use crate::{Epoch, MediaTimestamp, ProtocolError, Result, SeqNum, PROTOCOL_VERSION};

use super::PacketType;

/// Wire size of a media header.
pub const HEADER_LEN: usize = 23;

/// Set when a packet has been forwarded by a relay.
///
/// A relay refuses to forward a packet that already carries it, which is a
/// one-line defence against a forwarding loop between two nodes that each
/// believe the other is the relay — a state that is entirely reachable during
/// an election.
pub const FLAG_RELAYED: u8 = 0b0000_0001;

/// Set on the first packet of a talkspurt after VAD silence.
///
/// The receiver uses it to reset playout timing rather than trying to conceal a
/// gap that was never loss in the first place.
pub const FLAG_TALKSPURT_START: u8 = 0b0000_0010;

/// The relay-visible part of a media packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaHeader {
    /// Wire version.
    pub version: u8,
    /// Packet type.
    pub packet_type: PacketType,
    /// Flags.
    pub flags: u8,
    /// Truncated room id.
    pub room_route_id: u32,
    /// Truncated sender id.
    pub sender_route_id: u32,
    /// Stream within the sender.
    pub stream_id: u16,
    /// Per-stream sequence number.
    pub sequence: SeqNum,
    /// Sender media clock.
    pub timestamp: MediaTimestamp,
    /// Key generation, truncated. Wraps at 65536 membership changes, which is
    /// not a room that exists.
    pub epoch: u16,
}

impl MediaHeader {
    /// Build a header for an outgoing packet.
    #[must_use]
    pub fn new(
        packet_type: PacketType,
        room_route_id: u32,
        sender_route_id: u32,
        stream_id: u16,
        sequence: SeqNum,
        timestamp: MediaTimestamp,
        epoch: Epoch,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            packet_type,
            flags: 0,
            room_route_id,
            sender_route_id,
            stream_id,
            sequence,
            timestamp,
            epoch: epoch.0 as u16,
        }
    }

    /// Whether a relay has already forwarded this packet.
    #[must_use]
    pub const fn is_relayed(&self) -> bool {
        self.flags & FLAG_RELAYED != 0
    }

    /// Whether this packet starts a talkspurt.
    #[must_use]
    pub const fn is_talkspurt_start(&self) -> bool {
        self.flags & FLAG_TALKSPURT_START != 0
    }

    /// Mark as relayed. Returns false if it already was — the caller must drop
    /// the packet rather than forward it a second time.
    pub fn mark_relayed(&mut self) -> bool {
        if self.is_relayed() {
            return false;
        }
        self.flags |= FLAG_RELAYED;
        true
    }

    /// Serialise, big-endian throughout.
    #[must_use]
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0] = self.version;
        out[1] = self.packet_type.into();
        out[2] = self.flags;
        out[3..7].copy_from_slice(&self.room_route_id.to_be_bytes());
        out[7..11].copy_from_slice(&self.sender_route_id.to_be_bytes());
        out[11..13].copy_from_slice(&self.stream_id.to_be_bytes());
        out[13..17].copy_from_slice(&self.sequence.0.to_be_bytes());
        out[17..21].copy_from_slice(&self.timestamp.0.to_be_bytes());
        out[21..23].copy_from_slice(&self.epoch.to_be_bytes());
        out
    }

    /// Parse a header from the front of a datagram.
    ///
    /// Hostile input by definition — anyone within radio range can send bytes
    /// at this function. It never indexes without a length check and never
    /// panics.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(ProtocolError::Truncated { got: bytes.len(), need: HEADER_LEN }.into());
        }

        let version = bytes[0];
        if version != PROTOCOL_VERSION {
            return Err(
                ProtocolError::VersionMismatch { theirs: version, ours: PROTOCOL_VERSION }.into()
            );
        }

        Ok(Self {
            version,
            packet_type: PacketType::try_from(bytes[1])?,
            flags: bytes[2],
            room_route_id: u32::from_be_bytes(bytes[3..7].try_into().expect("checked length")),
            sender_route_id: u32::from_be_bytes(bytes[7..11].try_into().expect("checked length")),
            stream_id: u16::from_be_bytes(bytes[11..13].try_into().expect("checked length")),
            sequence: SeqNum(u32::from_be_bytes(bytes[13..17].try_into().expect("checked length"))),
            timestamp: MediaTimestamp(u32::from_be_bytes(
                bytes[17..21].try_into().expect("checked length"),
            )),
            epoch: u16::from_be_bytes(bytes[21..23].try_into().expect("checked length")),
        })
    }

    /// The header bytes, used as AEAD associated data.
    ///
    /// This is what binds the visible routing fields to the encrypted payload:
    /// a relay can *read* the sequence number and epoch, but changing either
    /// makes authentication fail at the receiver. That is what stops a
    /// malicious relay from re-sequencing a stream to force a replay or to
    /// scramble playout order (§79).
    #[must_use]
    pub fn associated_data(&self) -> [u8; HEADER_LEN] {
        // The relayed flag is deliberately excluded from the AAD by zeroing it
        // here: a relay must be able to set it without invalidating the tag,
        // since only the endpoints hold the key.
        let mut aad = self.encode();
        aad[2] &= !FLAG_RELAYED;
        aad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MediaHeader {
        MediaHeader::new(
            PacketType::Media,
            0xdead_beef,
            0x0123_4567,
            3,
            SeqNum(9_001),
            MediaTimestamp(48_000),
            Epoch(42),
        )
    }

    #[test]
    fn round_trips() {
        let header = sample();
        assert_eq!(MediaHeader::decode(&header.encode()).unwrap(), header);
    }

    #[test]
    fn header_is_the_documented_size() {
        assert_eq!(sample().encode().len(), 23);
    }

    #[test]
    fn decodes_with_a_payload_following() {
        let mut bytes = sample().encode().to_vec();
        bytes.extend_from_slice(&[0xAA; 64]);
        assert_eq!(MediaHeader::decode(&bytes).unwrap(), sample());
    }

    #[test]
    fn rejects_every_truncation() {
        let full = sample().encode();
        for cut in 0..HEADER_LEN {
            assert!(MediaHeader::decode(&full[..cut]).is_err(), "accepted a {cut}-byte header");
        }
    }

    #[test]
    fn rejects_wrong_version_and_unknown_type() {
        let mut bytes = sample().encode();
        bytes[0] = 7;
        assert!(MediaHeader::decode(&bytes).is_err());

        let mut bytes = sample().encode();
        bytes[1] = 0x77;
        assert!(MediaHeader::decode(&bytes).is_err());
    }

    #[test]
    fn relay_marking_is_one_way() {
        let mut header = sample();
        assert!(!header.is_relayed());
        assert!(header.mark_relayed());
        assert!(header.is_relayed());
        // Second relay must refuse: this is the forwarding-loop guard.
        assert!(!header.mark_relayed());
    }

    #[test]
    fn relay_flag_does_not_affect_associated_data() {
        // A relay must be able to set the flag without breaking the endpoints'
        // authentication tag.
        let plain = sample();
        let mut relayed = sample();
        relayed.mark_relayed();

        assert_ne!(plain.encode(), relayed.encode());
        assert_eq!(plain.associated_data(), relayed.associated_data());
    }

    #[test]
    fn associated_data_covers_sequence_and_epoch() {
        // Changing either must change the AAD, so tampering fails the tag.
        let base = sample();

        let mut resequenced = sample();
        resequenced.sequence = SeqNum(9_002);
        assert_ne!(base.associated_data(), resequenced.associated_data());

        let mut re_epoched = sample();
        re_epoched.epoch = 41;
        assert_ne!(base.associated_data(), re_epoched.associated_data());
    }

    #[test]
    fn never_panics_on_arbitrary_bytes() {
        for len in 0..64usize {
            for seed in 0..8u8 {
                let bytes: Vec<u8> =
                    (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect();
                let _ = MediaHeader::decode(&bytes);
            }
        }
    }
}
