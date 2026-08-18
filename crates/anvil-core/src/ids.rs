//! Identifiers.
//!
//! Every identifier here is deliberately *not* a network address. An IP moves
//! when the router changes; a Wi-Fi Aware peer handle is meaningless five
//! seconds later. Anvil identifies things by value so that the room survives
//! the network underneath it.

use core::fmt;

/// Stable identity of a device installation.
///
/// Derived from the device's long-lived Ed25519 public identity key (see
/// [`crate::crypto::identity`]), *not* from an IP, MAC or Wi-Fi Aware handle.
/// This is what makes discovery de-duplication possible: the same peer found
/// once over LAN and once over Wi-Fi Aware produces one `PeerId` with two
/// paths, not two participants.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PeerId(pub [u8; 32]);

impl PeerId {
    /// All-zero id. Useful as a "not yet known" sentinel in local state only —
    /// it must never be sent on the wire.
    pub const UNSPECIFIED: Self = Self([0u8; 32]);

    /// Raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Short form used in logs and the diagnostics UI (first 4 bytes, hex).
    ///
    /// Never use this for equality or routing — it is a display convenience and
    /// collides by design.
    #[must_use]
    pub fn short(&self) -> String {
        hex8(&self.0[..4])
    }

    /// Truncated routing identifier placed in packet headers.
    ///
    /// A relay needs *something* to route on but has no business seeing full
    /// participant identity for every packet. See `protocol/packet-format.md`
    /// for the metadata trade-off this represents.
    #[must_use]
    pub fn route_id(&self) -> u32 {
        u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
}

impl fmt::Debug for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PeerId({})", self.short())
    }
}

impl fmt::Display for PeerId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Identity of a room.
///
/// Cryptographically random, generated locally by whoever creates the room. It
/// must not encode the creator, the relay, an IP, or anything else that changes
/// — a room whose id depends on its relay cannot survive relay failover.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RoomId(pub [u8; 16]);

impl RoomId {
    /// Generate a fresh random room id.
    #[must_use]
    pub fn generate() -> Self {
        let mut bytes = [0u8; 16];
        rand_core::RngCore::fill_bytes(&mut rand_core::OsRng, &mut bytes);
        Self(bytes)
    }

    /// Raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    /// Short form for UI and logs.
    #[must_use]
    pub fn short(&self) -> String {
        hex8(&self.0[..3]).to_uppercase()
    }

    /// Truncated routing identifier placed in packet headers.
    #[must_use]
    pub fn route_id(&self) -> u32 {
        u32::from_be_bytes([self.0[0], self.0[1], self.0[2], self.0[3]])
    }
}

impl fmt::Debug for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "RoomId({})", self.short())
    }
}

impl fmt::Display for RoomId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Identity of one media stream.
///
/// Distinct from [`PeerId`] because a participant may eventually publish more
/// than one stream (screen audio, a second mic, a translation track). Keeping
/// them separate now costs nothing and avoids a packet format change later.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamId(pub u32);

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stream:{}", self.0)
    }
}

/// A transport path instance.
///
/// Locally scoped and never sent on the wire. Two paths to the same peer (LAN
/// and Wi-Fi Aware) get different `PathId`s; the same path re-established after
/// a drop gets a new one, so stale metrics can never be attributed to a fresh
/// connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PathId(pub u64);

/// Group key generation.
///
/// Advances on every membership change (§50). A packet carries its epoch so a
/// receiver mid-rotation knows which key material to try, and so that a
/// participant who has left cannot decrypt anything sent after their departure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Epoch(pub u64);

impl Epoch {
    /// The next epoch.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0 + 1)
    }
}

impl fmt::Display for Epoch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "epoch:{}", self.0)
    }
}

/// Per-stream media sequence number.
///
/// Wraps. Comparison must always be done with [`SeqNum::is_newer_than`], never
/// with `<`, or playback breaks once every ~13 hours at 20 ms frames.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SeqNum(pub u32);

impl SeqNum {
    /// Next sequence number, wrapping.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// Wrap-aware "is this newer than that", RFC 1982 style serial comparison.
    #[must_use]
    pub const fn is_newer_than(self, other: Self) -> bool {
        let diff = self.0.wrapping_sub(other.0);
        diff != 0 && diff < (u32::MAX / 2)
    }

    /// Wrap-aware forward distance from `other` to `self`.
    ///
    /// Returns `None` when `self` is not newer than `other`.
    #[must_use]
    pub const fn distance_from(self, other: Self) -> Option<u32> {
        if self.is_newer_than(other) {
            Some(self.0.wrapping_sub(other.0))
        } else {
            None
        }
    }
}

fn hex8(bytes: &[u8]) -> String {
    use fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_ids_are_distinct() {
        assert_ne!(RoomId::generate(), RoomId::generate());
    }

    #[test]
    fn sequence_comparison_survives_wrap() {
        let high = SeqNum(u32::MAX - 2);
        let wrapped = high.next().next().next().next(); // wraps past zero

        assert!(wrapped.is_newer_than(high));
        assert!(!high.is_newer_than(wrapped));
        assert_eq!(wrapped.distance_from(high), Some(4));
        assert_eq!(high.distance_from(wrapped), None);
    }

    #[test]
    fn sequence_is_not_newer_than_itself() {
        let s = SeqNum(42);
        assert!(!s.is_newer_than(s));
        assert_eq!(s.distance_from(s), None);
    }

    #[test]
    fn route_id_is_derived_from_leading_bytes() {
        let peer = PeerId([0xde, 0xad, 0xbe, 0xef, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
                           0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(peer.route_id(), 0xdead_beef);
        assert_eq!(peer.short(), "deadbeef");
    }
}
