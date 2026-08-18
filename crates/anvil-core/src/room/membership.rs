//! Participants and admission (§67, §68).

use crate::time::Monotonic;
use crate::{PeerId, StreamId};

/// Someone in the room.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    /// Cryptographically confirmed identity.
    ///
    /// Unlike a discovered peer, a participant is always confirmed — admission
    /// happens after the handshake, so there is no unauthenticated state to
    /// represent here.
    pub peer_id: PeerId,
    /// Display name, as asserted by the holder of the identity key.
    pub display_name: String,
    /// Streams this participant is publishing.
    pub streams: Vec<StreamId>,
    /// When they joined.
    pub joined_at: Monotonic,
    /// Whether they are currently transmitting speech (VAD-driven).
    pub speaking: bool,
    /// Whether they have muted themselves.
    ///
    /// Advisory only. A muted participant simply stops sending; nothing here
    /// prevents them from sending, and the UI should not imply otherwise.
    pub muted: bool,
}

impl Participant {
    /// A newly admitted participant.
    #[must_use]
    pub fn new(peer_id: PeerId, display_name: String, joined_at: Monotonic) -> Self {
        Self {
            peer_id,
            display_name,
            streams: Vec::new(),
            joined_at,
            speaking: false,
            muted: false,
        }
    }
}

/// How a room admits people (§68).
///
/// No cloud, no accounts, no phone numbers — admission has to work between
/// devices that have never met, with no shared infrastructure. Both options
/// below rely on the participants being physically together, which is the one
/// trust anchor Anvil actually has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdmissionPolicy {
    /// The host approves each request explicitly.
    ///
    /// Simplest to reason about and hard to attack, but it needs the host to be
    /// looking at their phone — awkward when the host is the person talking.
    HostApproval,

    /// Anyone presenting the join code is admitted.
    ///
    /// Shown on the host's screen or as a QR code. Better UX for a group
    /// forming at once, at the cost that anyone who can read the screen —
    /// including over a shoulder, or in a photograph — can join.
    JoinCode {
        /// The code, compared in constant time to avoid leaking a prefix
        /// through timing.
        code: String,
    },

    /// Anyone nearby may join.
    ///
    /// For situations where the room is genuinely public. Should be visibly
    /// distinct in the UI, because it means exactly what it says.
    Open,
}

impl AdmissionPolicy {
    /// Whether a presented credential satisfies the policy.
    ///
    /// Returns `false` for [`Self::HostApproval`]: that policy is not decided
    /// by a credential at all, and the engine must route it to the user rather
    /// than treating a false here as a rejection.
    #[must_use]
    pub fn accepts(&self, credential: Option<&[u8]>) -> bool {
        match self {
            Self::Open => true,
            Self::HostApproval => false,
            Self::JoinCode { code } => {
                credential.is_some_and(|presented| constant_time_eq(presented, code.as_bytes()))
            }
        }
    }

    /// Whether admission requires asking the user.
    #[must_use]
    pub const fn needs_user_approval(&self) -> bool {
        matches!(self, Self::HostApproval)
    }
}

/// Compare byte strings without leaking their common prefix length through
/// timing.
///
/// A join code is low-entropy enough that a timing oracle on the comparison is
/// a real shortcut — this is cheap insurance.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_rooms_admit_anyone() {
        assert!(AdmissionPolicy::Open.accepts(None));
        assert!(!AdmissionPolicy::Open.needs_user_approval());
    }

    #[test]
    fn host_approval_defers_to_the_user_rather_than_rejecting() {
        let policy = AdmissionPolicy::HostApproval;
        assert!(policy.needs_user_approval());
        assert!(!policy.accepts(Some(b"anything")));
    }

    #[test]
    fn join_codes_must_match_exactly() {
        let policy = AdmissionPolicy::JoinCode { code: "742913".into() };

        assert!(policy.accepts(Some(b"742913")));
        assert!(!policy.accepts(Some(b"742914")));
        assert!(!policy.accepts(Some(b"74291")));
        assert!(!policy.accepts(Some(b"7429130")));
        assert!(!policy.accepts(None));
    }

    #[test]
    fn constant_time_comparison_is_still_correct() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[test]
    fn a_new_participant_starts_silent_and_unmuted() {
        let p = Participant::new(PeerId([1u8; 32]), "Alice".into(), Monotonic(500));
        assert!(!p.speaking);
        assert!(!p.muted);
        assert!(p.streams.is_empty());
    }
}
