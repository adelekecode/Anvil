//! Room-internal state transitions.
//!
//! Distinct from [`crate::Event`], which is what the *host application* sees.
//! These are the transitions the engine applies to room state; several of them
//! produce no user-visible event at all, and one user-visible event can be the
//! result of several.
//!
//! Keeping the two separate stops the UI's needs from leaking into the
//! protocol's state machine — the day they merge is the day someone adds a
//! field to a protocol transition because a screen wanted it.

use crate::{Epoch, PeerId};

/// A change to room state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RoomTransition {
    /// A member was admitted.
    MemberAdded {
        /// Who.
        peer: PeerId,
        /// Epoch produced by the change.
        epoch: Epoch,
    },
    /// A member left or was removed.
    MemberRemoved {
        /// Who.
        peer: PeerId,
        /// Epoch produced by the change.
        epoch: Epoch,
    },
    /// The relay changed.
    RelayChanged {
        /// The new relay, or `None` while an election runs.
        relay: Option<PeerId>,
    },
    /// A member started or stopped speaking.
    SpeakingChanged {
        /// Who.
        peer: PeerId,
        /// Whether they are speaking now.
        speaking: bool,
    },
    /// Keys rotated without a membership change — periodic rotation, or
    /// recovery after suspected key loss.
    EpochAdvanced {
        /// The new epoch.
        epoch: Epoch,
    },
}

impl RoomTransition {
    /// Whether this transition requires new key material to be distributed.
    #[must_use]
    pub const fn requires_rekey(&self) -> bool {
        matches!(
            self,
            Self::MemberAdded { .. } | Self::MemberRemoved { .. } | Self::EpochAdvanced { .. }
        )
    }

    /// Whether this transition changes where media should be sent.
    #[must_use]
    pub const fn requires_reroute(&self) -> bool {
        matches!(self, Self::RelayChanged { .. } | Self::MemberAdded { .. }
                     | Self::MemberRemoved { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn membership_changes_always_rekey() {
        // §50: this is what makes departure mean anything.
        assert!(RoomTransition::MemberAdded { peer: PeerId::UNSPECIFIED, epoch: Epoch(1) }
            .requires_rekey());
        assert!(RoomTransition::MemberRemoved { peer: PeerId::UNSPECIFIED, epoch: Epoch(2) }
            .requires_rekey());
    }

    #[test]
    fn a_relay_change_reroutes_but_does_not_rekey() {
        // The relay holds no media keys, so replacing it changes nothing
        // cryptographic (§33). Rekeying here would be pure cost.
        let transition = RoomTransition::RelayChanged { relay: Some(PeerId::UNSPECIFIED) };
        assert!(transition.requires_reroute());
        assert!(!transition.requires_rekey());
    }

    #[test]
    fn speaking_changes_are_cosmetic() {
        let transition =
            RoomTransition::SpeakingChanged { peer: PeerId::UNSPECIFIED, speaking: true };
        assert!(!transition.requires_rekey());
        assert!(!transition.requires_reroute());
    }
}
