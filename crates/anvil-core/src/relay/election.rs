//! Relay election (§37–§40).
//!
//! One participant forwards media for the room. Which one is decided by score,
//! and the scoring here is a smaller problem than it looks — but the *stability*
//! of the decision is a much bigger one.
//!
//! ## Why flapping is the real enemy
//!
//! Changing relay is expensive: every participant re-points its media, packets
//! in flight to the old relay are lost, and the room glitches. Two devices with
//! near-identical scores that swap the role back and forth every few seconds
//! would produce a call that is audibly worse than either one relaying badly.
//!
//! Three defences, all of which have to hold simultaneously:
//!
//! 1. **Hysteresis** — a challenger must beat the incumbent by
//!    [`crate::RelayConfig::election_hysteresis`], a wider margin than the
//!    transport layer uses, because the switch costs more.
//! 2. **Minimum term** — a relay holds the role for
//!    [`crate::RelayConfig::min_term`] before any voluntary challenge.
//! 3. **Deterministic tie-breaks** — equal scores resolve by [`PeerId`]
//!    ordering, which every participant computes identically. Without this,
//!    two nodes can each conclude they won.
//!
//! Failure bypasses all three (§40): a dead relay is replaced immediately.
//!
//! ## Trusting self-reported scores
//!
//! Candidates report their own scores, and a malicious device can lie to
//! capture the role. That is worth stating plainly rather than hiding: what it
//! buys the attacker is the ability to drop, delay and observe metadata — all
//! of which a participant can largely do anyway — and *nothing* cryptographic
//! (§33, §79). The relay role confers no key material. A liar becomes a bad
//! relay, health monitoring notices, and an election removes them.

use crate::time::Monotonic;
use crate::{PeerId, RelayConfig};

/// A candidate's self-reported suitability (§38).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RelayCandidate {
    /// Who.
    pub peer: PeerId,
    /// How many room members this candidate can currently reach directly.
    ///
    /// The most important input by far: a relay that cannot reach everyone is
    /// not a relay, however fast its network is.
    pub reachable_members: u8,
    /// Mean path quality to those members, 0–100.
    pub network_quality: f32,
    /// Path stability, 0–100.
    pub stability: f32,
    /// Device capability headroom, 0–100 — CPU, thermal state.
    pub capability: f32,
    /// Battery percentage, if known.
    pub battery_pct: Option<u8>,
    /// Whether on external power.
    pub charging: bool,
}

impl RelayCandidate {
    /// Suitability score. Higher wins.
    ///
    /// Reachability dominates, then quality and stability, with a power penalty
    /// that a charging device escapes entirely — a plugged-in phone is simply
    /// the right answer when there is one.
    #[must_use]
    pub fn score(&self, total_members: u8, config: &RelayConfig) -> f32 {
        let reachable = f32::from(self.reachable_members);
        let total = f32::from(total_members.max(1));
        let connectivity = (reachable / total) * 100.0;

        // A candidate that cannot reach everyone is disqualified outright rather
        // than merely penalised: electing it partitions the room.
        if self.reachable_members < total_members {
            return 0.0;
        }

        // A device on external power is simply the right answer when there is
        // one: relaying is the most expensive job in the room, and a plugged-in
        // phone pays nothing for it. The bonus is small enough that it never
        // overrides connectivity or a large quality gap.
        let charging_bonus = if self.charging { 5.0 } else { 0.0 };

        let power_penalty = if self.charging {
            0.0
        } else {
            match self.battery_pct {
                Some(pct) if pct < config.battery_floor_pct => {
                    if config.hard_battery_floor {
                        return 0.0;
                    }
                    40.0
                }
                Some(pct) if pct < 40 => 15.0,
                Some(_) => 0.0,
                None => 5.0, // unknown battery is mildly suspicious
            }
        };

        let score = 0.40 * connectivity
            + 0.25 * self.network_quality.clamp(0.0, 100.0)
            + 0.20 * self.stability.clamp(0.0, 100.0)
            + 0.15 * self.capability.clamp(0.0, 100.0)
            + charging_bonus
            - power_penalty;

        score.clamp(0.0, 100.0)
    }
}

/// Why an election happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElectionReason {
    /// No relay yet.
    Bootstrap,
    /// The relay stopped responding (§41).
    RelayFailed,
    /// A challenger is meaningfully better.
    BetterCandidate,
    /// The relay stood down — battery, backgrounding, leaving.
    RelayResigned,
}

/// Outcome of an election round.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ElectionResult {
    /// The winner.
    pub relay: PeerId,
    /// Their score.
    pub score: f32,
    /// Why the election ran.
    pub reason: ElectionReason,
}

/// Run an election.
///
/// Returns `None` when the incumbent should keep the role, so "no change" is
/// explicit rather than inferred from an unchanged winner.
#[must_use]
pub fn elect(
    candidates: &[RelayCandidate],
    incumbent: Option<PeerId>,
    incumbent_since: Monotonic,
    incumbent_failed: bool,
    total_members: u8,
    config: &RelayConfig,
    now: Monotonic,
) -> Option<ElectionResult> {
    // Rank by score, tie-broken by PeerId so every participant reaches the same
    // answer independently. Without this, two nodes with equal scores can each
    // elect themselves and the room splits.
    let mut ranked: Vec<(&RelayCandidate, f32)> =
        candidates.iter().map(|c| (c, c.score(total_members, config))).collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.peer.cmp(&b.0.peer)));

    let (best, best_score) = *ranked.first()?;
    if best_score <= 0.0 {
        return None; // nobody can reach everybody
    }

    let Some(incumbent_id) = incumbent else {
        return Some(ElectionResult {
            relay: best.peer,
            score: best_score,
            reason: ElectionReason::Bootstrap,
        });
    };

    if incumbent_failed {
        // Hard failure bypasses hysteresis and term entirely (§40).
        return Some(ElectionResult {
            relay: best.peer,
            score: best_score,
            reason: ElectionReason::RelayFailed,
        });
    }

    if best.peer == incumbent_id {
        return None;
    }

    if now.saturating_since(incumbent_since) < config.min_term {
        return None;
    }

    let incumbent_score =
        ranked.iter().find(|(c, _)| c.peer == incumbent_id).map_or(0.0, |(_, score)| *score);

    if best_score - incumbent_score >= config.election_hysteresis {
        Some(ElectionResult {
            relay: best.peer,
            score: best_score,
            reason: ElectionReason::BetterCandidate,
        })
    } else {
        None
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

    fn candidate(n: u8, quality: f32, reachable: u8) -> RelayCandidate {
        RelayCandidate {
            peer: peer(n),
            reachable_members: reachable,
            network_quality: quality,
            stability: 90.0,
            capability: 80.0,
            battery_pct: Some(80),
            charging: false,
        }
    }

    #[test]
    fn bootstrap_elects_the_best_candidate() {
        let config = RelayConfig::default();
        let candidates = [candidate(1, 60.0, 3), candidate(2, 95.0, 3), candidate(3, 70.0, 3)];

        let result = elect(&candidates, None, Monotonic::ZERO, false, 3, &config, Monotonic(100))
            .expect("an election should have produced a relay");

        assert_eq!(result.relay, peer(2));
        assert_eq!(result.reason, ElectionReason::Bootstrap);
    }

    #[test]
    fn a_candidate_that_cannot_reach_everyone_is_disqualified() {
        let config = RelayConfig::default();
        // Peer 2 looks perfect but only reaches two of three members.
        let candidates = [candidate(1, 50.0, 3), candidate(2, 100.0, 2)];

        let result =
            elect(&candidates, None, Monotonic::ZERO, false, 3, &config, Monotonic(100)).unwrap();

        assert_eq!(result.relay, peer(1), "elected a relay that would partition the room");
    }

    #[test]
    fn no_election_when_nobody_can_reach_everybody() {
        let config = RelayConfig::default();
        let candidates = [candidate(1, 90.0, 2), candidate(2, 90.0, 1)];

        assert!(
            elect(&candidates, None, Monotonic::ZERO, false, 3, &config, Monotonic(0)).is_none()
        );
    }

    #[test]
    fn a_marginally_better_challenger_does_not_unseat_the_incumbent() {
        // The flapping case.
        let config = RelayConfig::default();
        let candidates = [candidate(1, 80.0, 3), candidate(2, 85.0, 3)];

        let result = elect(
            &candidates,
            Some(peer(1)),
            Monotonic::ZERO,
            false,
            3,
            &config,
            Monotonic(120_000),
        );

        assert!(result.is_none(), "relay changed for a marginal improvement");
    }

    #[test]
    fn a_much_better_challenger_wins_after_the_minimum_term() {
        let config = RelayConfig::default();
        let mut strong = candidate(2, 100.0, 3);
        strong.charging = true;
        strong.stability = 100.0;
        strong.capability = 100.0;
        let candidates = [candidate(1, 10.0, 3), strong];

        let result = elect(
            &candidates,
            Some(peer(1)),
            Monotonic::ZERO,
            false,
            3,
            &config,
            Monotonic(120_000),
        )
        .expect("a clearly better candidate should win");

        assert_eq!(result.relay, peer(2));
        assert_eq!(result.reason, ElectionReason::BetterCandidate);
    }

    #[test]
    fn the_minimum_term_protects_a_fresh_relay() {
        let config = RelayConfig::default();
        let mut strong = candidate(2, 100.0, 3);
        strong.charging = true;
        let candidates = [candidate(1, 10.0, 3), strong];

        // Elected five seconds ago; min_term is 30.
        let result =
            elect(&candidates, Some(peer(1)), Monotonic(0), false, 3, &config, Monotonic(5_000));

        assert!(result.is_none());
    }

    #[test]
    fn a_failed_relay_is_replaced_immediately() {
        // §41: Chris is the relay and disappears.
        let config = RelayConfig::default();
        let candidates = [candidate(1, 80.0, 3), candidate(2, 70.0, 3)];

        let result =
            elect(&candidates, Some(peer(3)), Monotonic(0), true, 3, &config, Monotonic(1_000))
                .expect("a dead relay must be replaced regardless of term");

        assert_eq!(result.reason, ElectionReason::RelayFailed);
        assert_eq!(result.relay, peer(1));
    }

    #[test]
    fn ties_break_deterministically_so_every_device_agrees() {
        // Two identical candidates. Every participant must pick the same one,
        // independently, or the room splits.
        let config = RelayConfig::default();
        let a = candidate(7, 90.0, 3);
        let b = candidate(2, 90.0, 3);

        let forward = elect(&[a, b], None, Monotonic::ZERO, false, 3, &config, Monotonic(0));
        let reversed = elect(&[b, a], None, Monotonic::ZERO, false, 3, &config, Monotonic(0));

        assert_eq!(forward.unwrap().relay, reversed.unwrap().relay);
        assert_eq!(forward.unwrap().relay, peer(2), "expected the lower PeerId to win");
    }

    #[test]
    fn a_charging_device_beats_an_equal_one_on_battery() {
        let config = RelayConfig::default();
        let mut plugged_in = candidate(5, 80.0, 3);
        plugged_in.charging = true;
        plugged_in.battery_pct = Some(30);

        let on_battery = candidate(1, 80.0, 3); // 80%, not charging, lower PeerId

        let result = elect(
            &[on_battery, plugged_in],
            None,
            Monotonic::ZERO,
            false,
            3,
            &config,
            Monotonic(0),
        );

        assert_eq!(result.unwrap().relay, peer(5));
    }

    #[test]
    fn a_nearly_flat_phone_is_avoided_but_can_still_serve_if_alone() {
        let config = RelayConfig::default(); // hard_battery_floor = false
        let mut flat = candidate(1, 90.0, 2);
        flat.battery_pct = Some(6);

        let result = elect(&[flat], None, Monotonic::ZERO, false, 2, &config, Monotonic(0));

        assert!(result.is_some(), "losing the room is worse than relaying at 6%");
    }

    #[test]
    fn a_hard_battery_floor_disqualifies_outright() {
        let config = RelayConfig { hard_battery_floor: true, ..RelayConfig::default() };
        let mut flat = candidate(1, 90.0, 2);
        flat.battery_pct = Some(6);

        assert!(elect(&[flat], None, Monotonic::ZERO, false, 2, &config, Monotonic(0)).is_none());
    }
}
