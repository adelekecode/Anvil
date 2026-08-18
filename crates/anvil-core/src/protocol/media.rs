//! Media packets on the wire (§52, §55).
//!
//! ```text
//!   ┌──────────────────────────┐
//!   │ MediaHeader (23 bytes)   │  ← relay-visible, authenticated as AAD
//!   ├──────────────────────────┤
//!   │ ciphertext               │  ← encrypted Opus frame
//!   │ AEAD tag (16 bytes)      │  ← appended by the AEAD
//!   └──────────────────────────┘
//! ```
//!
//! This type performs no cryptography. It frames bytes and hands them on. The
//! separation is deliberate: parsing hostile input and handling key material
//! are different jobs with different failure modes, and mixing them is how
//! parsers end up with keys in scope.

use super::header::{MediaHeader, HEADER_LEN};
use crate::{ProtocolError, Result};

/// Size of the AEAD authentication tag (ChaCha20-Poly1305 / AES-GCM).
pub const TAG_LEN: usize = 16;

/// A framed media packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaPacket {
    /// Relay-visible routing header.
    pub header: MediaHeader,
    /// Encrypted Opus frame with its tag appended. Opaque here.
    pub ciphertext: Vec<u8>,
}

impl MediaPacket {
    /// Frame a header and ciphertext for sending.
    #[must_use]
    pub fn new(header: MediaHeader, ciphertext: Vec<u8>) -> Self {
        Self { header, ciphertext }
    }

    /// Serialise.
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_LEN + self.ciphertext.len());
        out.extend_from_slice(&self.header.encode());
        out.extend_from_slice(&self.ciphertext);
        out
    }

    /// Parse a received datagram.
    ///
    /// A packet with no room for a tag is rejected here rather than in the
    /// crypto layer, so that obviously-malformed traffic never reaches code
    /// that holds keys.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let header = MediaHeader::decode(bytes)?;
        let body = &bytes[HEADER_LEN..];

        if body.len() < TAG_LEN {
            return Err(
                ProtocolError::Truncated { got: bytes.len(), need: HEADER_LEN + TAG_LEN }.into()
            );
        }

        Ok(Self { header, ciphertext: body.to_vec() })
    }

    /// Total wire size.
    #[must_use]
    pub fn wire_len(&self) -> usize {
        HEADER_LEN + self.ciphertext.len()
    }
}

/// Largest Opus frame that fits in one datagram on a path.
///
/// The audio encoder is sized from this rather than from a fixed constant,
/// because Wi-Fi Aware and LAN do not agree on datagram size and an Opus frame
/// must never be split — one lost fragment would destroy a frame the decoder
/// could otherwise have concealed.
#[must_use]
pub fn max_opus_payload(max_datagram_size: usize) -> usize {
    max_datagram_size.saturating_sub(HEADER_LEN + TAG_LEN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PacketType;
    use crate::{Epoch, MediaTimestamp, SeqNum};

    fn header() -> MediaHeader {
        MediaHeader::new(PacketType::Media, 1, 2, 0, SeqNum(7), MediaTimestamp(960), Epoch(1))
    }

    #[test]
    fn round_trips() {
        let packet = MediaPacket::new(header(), vec![0xAB; 80]);
        assert_eq!(MediaPacket::decode(&packet.encode()).unwrap(), packet);
    }

    #[test]
    fn rejects_a_packet_too_small_to_hold_a_tag() {
        let packet = MediaPacket::new(header(), vec![0xAB; TAG_LEN - 1]);
        assert!(MediaPacket::decode(&packet.encode()).is_err());
    }

    #[test]
    fn payload_budget_leaves_room_for_header_and_tag() {
        let budget = max_opus_payload(1_200);
        assert_eq!(budget, 1_200 - HEADER_LEN - TAG_LEN);

        // A frame filling the budget must still fit the datagram.
        let packet = MediaPacket::new(header(), vec![0u8; budget + TAG_LEN]);
        assert_eq!(packet.wire_len(), 1_200);
    }

    #[test]
    fn a_tiny_datagram_yields_no_budget_rather_than_underflowing() {
        assert_eq!(max_opus_payload(10), 0);
    }

    #[test]
    fn a_typical_voice_frame_fits_the_smallest_path() {
        // 20ms of 24kbps Opus is ~60 bytes; overhead is 39.
        let aware = crate::transport::aware::CONSERVATIVE_DATAGRAM_SIZE;
        assert!(max_opus_payload(aware) > 60);
    }
}
