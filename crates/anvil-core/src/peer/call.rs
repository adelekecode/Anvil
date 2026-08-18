//! Direct call state.
//!
//! A one-to-one call needs no relay and no room — two people is the case where
//! Anvil's topology is simplest (§35). What it does need is a state machine
//! strict enough that the two devices cannot end up disagreeing about whether
//! they are in a call.
//!
//! ```text
//!            ┌──────┐
//!      ┌─────│ Idle │◄────────────┐
//!      │     └──────┘             │
//!  place│         ▲ ring received │
//!      ▼         │                │
//!  ┌────────┐  ┌──────────┐       │
//!  │Outgoing│  │ Incoming │       │ decline / cancel /
//!  └────┬───┘  └────┬─────┘       │ hang up / timeout
//!       │ accepted  │ accept      │
//!       └─────┬─────┘             │
//!             ▼                   │
//!        ┌────────┐               │
//!        │ Active │───────────────┘
//!        └────────┘
//! ```
//!
//! ## Why this is a state machine and not two booleans
//!
//! The failure mode it prevents is the one users notice: a call that is "active"
//! on one phone and over on the other, so someone is talking to nobody. Every
//! transition here is guarded, and an invalid one is rejected rather than
//! applied — a device that receives an `Accept` it did not ask for stays where
//! it is instead of drifting into a state its peer is not in.
//!
//! ## Glare
//!
//! Two people can call each other at the same instant. Both sides then hold an
//! outgoing call and an incoming call from the same peer. Rather than both
//! failing politely — which is what naive implementations do, leaving two people
//! staring at "call failed" — [`CallState::resolve_glare`] settles it
//! deterministically by `PeerId`, so exactly one of the two calls survives and
//! the other side auto-accepts.

use core::time::Duration;

use crate::time::Monotonic;
use crate::PeerId;

/// How long an unanswered call rings before giving up.
///
/// Short by telephone standards on purpose: Anvil calls are between people in
/// the same building, and a phone ringing for a minute in a room where the
/// caller can see the callee is absurd.
pub const RING_TIMEOUT: Duration = Duration::from_secs(30);

/// Where a direct call has got to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallState {
    /// No call.
    #[default]
    Idle,
    /// We are calling someone and waiting.
    Outgoing {
        /// Who we called.
        peer: PeerId,
        /// When we placed it, for the ring timeout.
        since: Monotonic,
    },
    /// Someone is calling us.
    Incoming {
        /// Who is calling.
        peer: PeerId,
        /// When it started ringing.
        since: Monotonic,
    },
    /// Connected.
    Active {
        /// The other party.
        peer: PeerId,
        /// When it connected, for the duration display.
        since: Monotonic,
    },
}

/// Why a call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallEnded {
    /// The caller cancelled before it was answered.
    Cancelled,
    /// The callee declined.
    Declined,
    /// Either side hung up a connected call.
    HungUp,
    /// Nobody answered within [`RING_TIMEOUT`].
    Unanswered,
    /// The peer became unreachable.
    ///
    /// Distinct from `HungUp` because the UI should say different things:
    /// somebody hanging up is a decision, losing a path is a network event and
    /// worth showing as such.
    Unreachable,
}

/// Rejected because the state machine does not allow it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidTransition {
    /// What was attempted.
    pub attempted: &'static str,
    /// What state we were in.
    pub state: &'static str,
}

impl core::fmt::Display for InvalidTransition {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "cannot {} while {}", self.attempted, self.state)
    }
}

type Transition = Result<CallState, InvalidTransition>;

impl CallState {
    /// The other party, if there is one.
    #[must_use]
    pub const fn peer(&self) -> Option<PeerId> {
        match self {
            Self::Idle => None,
            Self::Outgoing { peer, .. }
            | Self::Incoming { peer, .. }
            | Self::Active { peer, .. } => Some(*peer),
        }
    }

    /// Whether a call is connected.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Active { .. })
    }

    /// Whether anything is happening — ringing either way, or connected.
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }

    /// Human-readable state name, for error messages and diagnostics.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Outgoing { .. } => "calling",
            Self::Incoming { .. } => "ringing",
            Self::Active { .. } => "in a call",
        }
    }

    /// Place a call.
    pub fn place(self, peer: PeerId, now: Monotonic) -> Transition {
        match self {
            Self::Idle => Ok(Self::Outgoing { peer, since: now }),
            other => Err(InvalidTransition { attempted: "place a call", state: other.name() }),
        }
    }

    /// A call arrived.
    ///
    /// Arriving while already busy is not an error in the machine — it is a
    /// normal thing that happens — but v0.1 has no call waiting, so it is
    /// refused and the caller is told, rather than interrupting a conversation
    /// in progress.
    pub fn ring(self, peer: PeerId, now: Monotonic) -> Transition {
        match self {
            Self::Idle => Ok(Self::Incoming { peer, since: now }),
            other => Err(InvalidTransition { attempted: "receive a call", state: other.name() }),
        }
    }

    /// Answer an incoming call.
    pub fn accept(self, now: Monotonic) -> Transition {
        match self {
            Self::Incoming { peer, .. } => Ok(Self::Active { peer, since: now }),
            other => Err(InvalidTransition { attempted: "accept", state: other.name() }),
        }
    }

    /// The callee accepted our outgoing call.
    pub fn accepted(self, now: Monotonic) -> Transition {
        match self {
            Self::Outgoing { peer, .. } => Ok(Self::Active { peer, since: now }),
            other => Err(InvalidTransition { attempted: "connect", state: other.name() }),
        }
    }

    /// End whatever is happening, returning the new state and why it ended.
    ///
    /// Deliberately total: hanging up must work from any state, including ones
    /// that should be impossible. A user pressing "end call" and having nothing
    /// happen is unforgivable, so this never fails.
    #[must_use]
    pub fn end(self, reason: CallEnded) -> (Self, Option<CallEnded>) {
        match self {
            Self::Idle => (Self::Idle, None),
            _ => (Self::Idle, Some(reason)),
        }
    }

    /// Whether an unanswered call has rung long enough to give up.
    #[must_use]
    pub fn has_timed_out(&self, now: Monotonic) -> bool {
        match self {
            Self::Outgoing { since, .. } | Self::Incoming { since, .. } => {
                now.saturating_since(*since) >= RING_TIMEOUT
            }
            _ => false,
        }
    }

    /// How long the current call has been connected.
    #[must_use]
    pub fn duration(&self, now: Monotonic) -> Option<Duration> {
        match self {
            Self::Active { since, .. } => Some(now.saturating_since(*since)),
            _ => None,
        }
    }

    /// Settle simultaneous calls between the same two people.
    ///
    /// Both sides run this with the same inputs and reach opposite, compatible
    /// conclusions: the peer with the lower [`PeerId`] keeps its outgoing call,
    /// and the other side accepts. Deterministic, so there is no negotiation
    /// round and no possibility of both sides deferring.
    ///
    /// Returns the state this device should move to.
    #[must_use]
    pub fn resolve_glare(self, local: PeerId, remote: PeerId, now: Monotonic) -> Self {
        match self {
            // We are calling them and they are calling us.
            Self::Outgoing { peer, .. } if peer == remote => {
                if local < remote {
                    // Our call wins; wait for their accept.
                    self
                } else {
                    // Theirs wins; treat it as answered.
                    Self::Active { peer: remote, since: now }
                }
            }
            other => other,
        }
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
    fn a_normal_outgoing_call_runs_its_course() {
        let state = CallState::Idle.place(peer(2), Monotonic(100)).unwrap();
        assert!(matches!(state, CallState::Outgoing { .. }));
        assert_eq!(state.peer(), Some(peer(2)));

        let state = state.accepted(Monotonic(3_000)).unwrap();
        assert!(state.is_active());
        assert_eq!(state.duration(Monotonic(63_000)), Some(Duration::from_secs(60)));

        let (state, reason) = state.end(CallEnded::HungUp);
        assert_eq!(state, CallState::Idle);
        assert_eq!(reason, Some(CallEnded::HungUp));
    }

    #[test]
    fn a_normal_incoming_call_runs_its_course() {
        let state = CallState::Idle.ring(peer(2), Monotonic(100)).unwrap();
        assert!(matches!(state, CallState::Incoming { .. }));

        let state = state.accept(Monotonic(2_000)).unwrap();
        assert!(state.is_active());
    }

    #[test]
    fn declining_returns_to_idle() {
        let state = CallState::Idle.ring(peer(2), Monotonic(100)).unwrap();
        let (state, reason) = state.end(CallEnded::Declined);

        assert_eq!(state, CallState::Idle);
        assert_eq!(reason, Some(CallEnded::Declined));
    }

    #[test]
    fn a_second_call_cannot_interrupt_one_in_progress() {
        let active = CallState::Idle
            .place(peer(2), Monotonic(100))
            .unwrap()
            .accepted(Monotonic(200))
            .unwrap();

        assert!(active.ring(peer(3), Monotonic(300)).is_err());
        assert!(active.place(peer(3), Monotonic(300)).is_err());
    }

    #[test]
    fn accepting_a_call_that_is_not_ringing_is_refused() {
        assert!(CallState::Idle.accept(Monotonic(100)).is_err());

        let outgoing = CallState::Idle.place(peer(2), Monotonic(100)).unwrap();
        assert!(outgoing.accept(Monotonic(200)).is_err());
    }

    #[test]
    fn hanging_up_always_works_even_from_idle() {
        // A user pressing "end call" must never have nothing happen.
        for state in [
            CallState::Idle,
            CallState::Outgoing { peer: peer(2), since: Monotonic(0) },
            CallState::Incoming { peer: peer(2), since: Monotonic(0) },
            CallState::Active { peer: peer(2), since: Monotonic(0) },
        ] {
            let (after, _) = state.end(CallEnded::HungUp);
            assert_eq!(after, CallState::Idle);
        }
    }

    #[test]
    fn ending_from_idle_reports_no_reason() {
        let (_, reason) = CallState::Idle.end(CallEnded::HungUp);
        assert_eq!(reason, None, "an idle device should not announce a hangup");
    }

    #[test]
    fn unanswered_calls_time_out_in_both_directions() {
        let outgoing = CallState::Idle.place(peer(2), Monotonic(1_000)).unwrap();
        let incoming = CallState::Idle.ring(peer(2), Monotonic(1_000)).unwrap();

        assert!(!outgoing.has_timed_out(Monotonic(10_000)));
        assert!(!incoming.has_timed_out(Monotonic(10_000)));

        let past = Monotonic(1_000 + RING_TIMEOUT.as_millis() as u64 + 1);
        assert!(outgoing.has_timed_out(past));
        assert!(incoming.has_timed_out(past));
    }

    #[test]
    fn a_connected_call_never_times_out() {
        let active = CallState::Active { peer: peer(2), since: Monotonic(0) };
        assert!(!active.has_timed_out(Monotonic(3_600_000)));
    }

    #[test]
    fn simultaneous_calls_resolve_to_one_call_not_two_failures() {
        // Femi is anv_01…, Daniel is anv_05…. Both call at once.
        let femi = peer(1);
        let daniel = peer(5);

        let femi_state = CallState::Idle.place(daniel, Monotonic(100)).unwrap().resolve_glare(
            femi,
            daniel,
            Monotonic(150),
        );
        let daniel_state = CallState::Idle.place(femi, Monotonic(100)).unwrap().resolve_glare(
            daniel,
            femi,
            Monotonic(150),
        );

        // Femi's call wins because his PeerId is lower; Daniel's side answers.
        assert!(matches!(femi_state, CallState::Outgoing { .. }));
        assert!(daniel_state.is_active());
        assert_eq!(daniel_state.peer(), Some(femi));
    }

    #[test]
    fn glare_resolution_ignores_unrelated_calls() {
        let state = CallState::Idle.place(peer(9), Monotonic(100)).unwrap();
        let after = state.resolve_glare(peer(1), peer(5), Monotonic(150));
        assert_eq!(after, state);
    }

    #[test]
    fn losing_the_peer_is_distinguishable_from_hanging_up() {
        let active = CallState::Active { peer: peer(2), since: Monotonic(0) };
        let (_, reason) = active.end(CallEnded::Unreachable);

        assert_eq!(reason, Some(CallEnded::Unreachable));
        assert_ne!(reason, Some(CallEnded::HungUp));
    }
}
