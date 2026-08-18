//! Counters and the diagnostics snapshot (§92).
//!
//! A diagnostics view exists from the first prototype because the failures this
//! system will actually have — a path that switches too often, a relay that
//! keeps losing an election, a jitter buffer that grew to its ceiling — are
//! invisible from the outside. All the user can report is "it sounded bad".
//!
//! ## What is deliberately absent
//!
//! No private keys, no media keys, no session secrets, no plaintext audio, no
//! peer identities beyond a short display form. This is enforced by the type:
//! the snapshot has no field capable of holding key material, so there is no
//! path by which a future change accidentally logs one.

use core::time::Duration;

use crate::transport::PathKind;
use crate::{Epoch, PeerId, RoomId};

/// Running counters for one node.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    /// Media packets sent.
    pub packets_sent: u64,
    /// Media packets received and authenticated.
    pub packets_received: u64,
    /// Packets dropped for failing authentication (§80).
    pub packets_rejected_auth: u64,
    /// Packets dropped as replays (§81).
    pub packets_rejected_replay: u64,
    /// Frames concealed by the decoder.
    pub frames_concealed: u64,
    /// Frames dropped for arriving after their playout slot.
    pub frames_late: u64,
    /// Packets forwarded while acting as relay.
    pub packets_forwarded: u64,
    /// Transport path switches.
    pub path_switches: u64,
    /// Relay changes.
    pub relay_changes: u64,
}

impl Counters {
    /// Fraction of received packets that were rejected.
    ///
    /// A number worth watching: sustained non-zero rejection with a working
    /// call usually means a bug in nonce or epoch handling, not an attacker.
    #[must_use]
    pub fn rejection_rate(&self) -> f32 {
        let rejected = self.packets_rejected_auth + self.packets_rejected_replay;
        let total = self.packets_received + rejected;
        if total == 0 {
            return 0.0;
        }
        rejected as f32 / total as f32
    }

    /// Fraction of expected frames that had to be concealed.
    #[must_use]
    pub fn conceal_rate(&self) -> f32 {
        let total = self.packets_received + self.frames_concealed;
        if total == 0 {
            return 0.0;
        }
        self.frames_concealed as f32 / total as f32
    }
}

/// Live view of one path.
#[derive(Clone, Copy, Debug)]
pub struct PathDiagnostics {
    /// Transport.
    pub kind: PathKind,
    /// Smoothed round-trip time.
    pub rtt: Duration,
    /// Smoothed loss fraction.
    pub loss: f32,
    /// Smoothed jitter.
    pub jitter: Duration,
    /// Current score.
    pub score: f32,
    /// Whether media is on this path.
    pub active: bool,
}

/// Everything the diagnostics screen shows.
#[derive(Clone, Debug)]
pub struct DiagnosticsSnapshot {
    /// This device, short form.
    pub local_peer: String,
    /// Current room, short form.
    pub room: Option<String>,
    /// Current relay, short form.
    pub relay: Option<String>,
    /// Whether this device is relaying.
    pub is_relay: bool,
    /// Key epoch. A number, not key material.
    pub epoch: Epoch,
    /// Participants.
    pub participant_count: usize,
    /// Per-peer paths.
    pub paths: Vec<PathDiagnostics>,
    /// Jitter buffer depth in use.
    pub jitter_depth: Duration,
    /// Opus bitrate in use.
    pub opus_bitrate_bps: u32,
    /// Counters.
    pub counters: Counters,
}

impl DiagnosticsSnapshot {
    /// Build a snapshot with only identifiers in their short, non-sensitive
    /// form.
    #[must_use]
    pub fn new(local: PeerId, room: Option<RoomId>, relay: Option<PeerId>) -> Self {
        Self {
            local_peer: local.short(),
            room: room.map(|r| r.short()),
            relay: relay.map(|r| r.short()),
            is_relay: relay == Some(local),
            epoch: Epoch(0),
            participant_count: 0,
            paths: Vec::new(),
            jitter_depth: Duration::ZERO,
            opus_bitrate_bps: 0,
            counters: Counters::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_zero_before_any_traffic() {
        let counters = Counters::default();
        assert_eq!(counters.rejection_rate(), 0.0);
        assert_eq!(counters.conceal_rate(), 0.0);
    }

    #[test]
    fn rejection_rate_counts_both_rejection_kinds() {
        let counters = Counters {
            packets_received: 90,
            packets_rejected_auth: 5,
            packets_rejected_replay: 5,
            ..Counters::default()
        };
        assert!((counters.rejection_rate() - 0.1).abs() < 0.001);
    }

    #[test]
    fn snapshots_carry_short_identifiers_only() {
        let local = PeerId([0xAB; 32]);
        let snapshot = DiagnosticsSnapshot::new(local, Some(RoomId([0xCD; 16])), Some(local));

        assert!(snapshot.is_relay);
        // Short forms, not full identifiers.
        assert_eq!(snapshot.local_peer.len(), 8);
        assert_eq!(snapshot.room.as_ref().unwrap().len(), 6);
    }

    #[test]
    fn a_snapshot_cannot_carry_key_material() {
        // Structural, not behavioural: the snapshot exposes counters, durations
        // and short strings. If a future change adds a Vec<u8> here, this
        // comment is the reason to push back.
        let snapshot = DiagnosticsSnapshot::new(PeerId::UNSPECIFIED, None, None);
        let rendered = format!("{snapshot:?}");
        assert!(!rendered.contains("key"), "{rendered}");
    }
}
