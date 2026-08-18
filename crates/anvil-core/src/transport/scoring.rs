//! Path scoring and the switch decision (§18, §19, §84, §85).
//!
//! Two pure functions, both fully testable without a network:
//!
//! * [`score_path`] turns measurements into a single 0–100 number.
//! * [`should_switch`] decides whether that number justifies moving a live call.
//!
//! Keeping these pure is not stylistic. Failover bugs are the hardest class of
//! bug in this system to reproduce on real hardware — they need two radios, a
//! router you can unplug, and luck. Being able to assert the decision directly
//! from a table of numbers is the difference between a testable system and a
//! hopeful one.

use core::time::Duration;

use super::{PathKind, PathMetrics};
use crate::config::{PathWeights, TransportConfig};
use crate::time::Monotonic;

/// Score a path, 0–100. Higher is better.
///
/// Each term is normalised to 0–100 independently, then weighted. The
/// normalisation curves matter more than the weights: latency and jitter use
/// piecewise-linear mappings anchored to what is audible in a voice call, not
/// to abstract "good/bad" thresholds.
#[must_use]
pub fn score_path(
    metrics: &PathMetrics,
    kind: PathKind,
    config: &TransportConfig,
    now: Monotonic,
) -> f32 {
    let w: &PathWeights = &config.weights;

    let weighted = w.latency * latency_score(metrics.rtt)
        + w.loss * loss_score(metrics.loss)
        + w.jitter * jitter_score(metrics.jitter)
        + w.stability * stability_score(metrics, now)
        + w.hops * hop_score(metrics.hops)
        + w.power * power_score(kind);

    let mut score = weighted / w.total().max(f32::EPSILON);

    // Static preference, applied only as a nudge (§19). Measured quality is
    // supposed to decide; this exists to break ties deterministically.
    if kind == PathKind::Lan {
        score += config.lan_preference_bonus;
    }

    // An unmeasured path is a guess. Penalise it so a live call is never moved
    // onto a path whose quality is still hypothetical — it can win once it has
    // actually been probed.
    if !metrics.measured {
        score -= 25.0;
    }

    score.clamp(0.0, 100.0)
}

/// Latency: flat 100 below 10 ms, then falling, worthless past 300 ms.
///
/// The flat region is intentional. On a LAN, 3 ms versus 7 ms is not a
/// difference any human perceives, and letting it move the score invites
/// switching between two indistinguishable paths.
fn latency_score(rtt: Duration) -> f32 {
    let ms = rtt.as_secs_f32() * 1000.0;
    match ms {
        m if m <= 10.0 => 100.0,
        m if m >= 300.0 => 0.0,
        m => 100.0 * (1.0 - (m - 10.0) / 290.0),
    }
}

/// Loss: steep. 1% is noticeable with Opus PLC, 5% is unpleasant, 20% is over.
fn loss_score(loss: f32) -> f32 {
    let pct = (loss * 100.0).max(0.0);
    match pct {
        p if p <= 0.5 => 100.0,
        p if p >= 20.0 => 0.0,
        p => 100.0 * (1.0 - ((p - 0.5) / 19.5).powf(0.6)),
    }
}

/// Jitter: what the jitter buffer must absorb. Past the buffer's ceiling
/// (§`AudioConfig::jitter_max`, 200 ms) the path is unusable for conversation.
fn jitter_score(jitter: Duration) -> f32 {
    let ms = jitter.as_secs_f32() * 1000.0;
    match ms {
        m if m <= 5.0 => 100.0,
        m if m >= 150.0 => 0.0,
        m => 100.0 * (1.0 - (m - 5.0) / 145.0),
    }
}

/// Stability: rewards uptime, punishes a history of disruption.
///
/// Uptime saturates at 60 seconds — a path that has held for a minute has
/// proven what it is going to prove, and letting the reward grow forever would
/// make an old mediocre path unbeatable by a new good one.
fn stability_score(metrics: &PathMetrics, now: Monotonic) -> f32 {
    let uptime_s = metrics.uptime(now).as_secs_f32();
    let uptime_component = (uptime_s / 60.0).min(1.0) * 100.0;
    let penalty = (metrics.disruptions as f32 * 15.0).min(75.0);
    (uptime_component - penalty).clamp(0.0, 100.0)
}

/// Hops: direct beats relayed (§19).
fn hop_score(hops: u8) -> f32 {
    match hops {
        0 => 100.0,
        1 => 70.0,
        _ => 40.0,
    }
}

/// Power: cheaper radio scores higher.
fn power_score(kind: PathKind) -> f32 {
    100.0 * (1.0 - kind.power_cost())
}

/// What the transport manager decided to do about the active path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchDecision {
    /// Stay put.
    Stay,
    /// Move voluntarily because the candidate is meaningfully better.
    Switch,
    /// Move immediately because the active path is dead. Hysteresis and dwell
    /// time do not apply (§85).
    Failover,
}

/// Decide whether to move media to `candidate`.
///
/// Three gates, in order of precedence:
///
/// 1. **Hard failure wins outright.** If the active path is stale, switch now.
///    Waiting out a dwell timer while a call is silent is indefensible.
/// 2. **Dwell time.** Otherwise, a path that was just adopted stays adopted for
///    `min_dwell`, no matter how attractive something else looks.
/// 3. **Hysteresis.** Finally, the candidate must beat the active path by
///    `switch_hysteresis`, not merely tie it (§84).
#[must_use]
pub fn should_switch(
    active_score: f32,
    active_metrics: &PathMetrics,
    active_since: Monotonic,
    candidate_score: f32,
    config: &TransportConfig,
    now: Monotonic,
) -> SwitchDecision {
    if active_metrics.is_stale(now, config.path_timeout) {
        return SwitchDecision::Failover;
    }

    if now.saturating_since(active_since) < config.min_dwell {
        return SwitchDecision::Stay;
    }

    if candidate_score - active_score >= config.switch_hysteresis {
        SwitchDecision::Switch
    } else {
        SwitchDecision::Stay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn measured(
        rtt_ms: u64,
        loss: f32,
        jitter_ms: u64,
        uptime_ms: u64,
    ) -> (PathMetrics, Monotonic) {
        let now = Monotonic(uptime_ms);
        let mut m = PathMetrics::new(Monotonic::ZERO, 0);
        m.rtt = Duration::from_millis(rtt_ms);
        m.loss = loss;
        m.jitter = Duration::from_millis(jitter_ms);
        m.measured = true;
        m.last_activity = now;
        (m, now)
    }

    #[test]
    fn the_spec_worked_example_prefers_lan() {
        // §18: LAN 3ms/0.1%/1ms vs Aware 7ms/0.3%/2ms, both stable.
        let config = TransportConfig::default();
        let (lan, now) = measured(3, 0.001, 1, 60_000);
        let (aware, _) = measured(7, 0.003, 2, 60_000);

        let lan_score = score_path(&lan, PathKind::Lan, &config, now);
        let aware_score = score_path(&aware, PathKind::WifiAware, &config, now);

        assert!(lan_score > aware_score, "lan {lan_score} aware {aware_score}");
        // ...but not by enough to justify tearing down a live Aware path.
        assert_eq!(
            should_switch(aware_score, &aware, Monotonic::ZERO, lan_score, &config, now),
            SwitchDecision::Stay,
            "near-equal paths must not trigger a switch"
        );
    }

    #[test]
    fn a_badly_degraded_active_path_loses_to_a_clean_one() {
        // §84: 55 vs 89 should switch.
        let config = TransportConfig::default();
        let (bad, now) = measured(120, 0.09, 60, 60_000);
        let (good, _) = measured(5, 0.001, 2, 60_000);

        let bad_score = score_path(&bad, PathKind::Lan, &config, now);
        let good_score = score_path(&good, PathKind::WifiAware, &config, now);

        assert!(
            good_score - bad_score >= config.switch_hysteresis,
            "bad {bad_score} good {good_score}"
        );
        assert_eq!(
            should_switch(bad_score, &bad, Monotonic::ZERO, good_score, &config, now),
            SwitchDecision::Switch
        );
    }

    #[test]
    fn dead_path_fails_over_immediately_ignoring_dwell_and_hysteresis() {
        let config = TransportConfig::default();
        let (mut dead, now) = measured(5, 0.0, 1, 60_000);
        dead.last_activity = Monotonic(0); // nothing for 60s

        // Adopted one second ago, and the candidate is *worse*. Still failover:
        // a worse working path beats a perfect dead one.
        let decision = should_switch(95.0, &dead, Monotonic(59_000), 40.0, &config, now);

        assert_eq!(decision, SwitchDecision::Failover);
    }

    #[test]
    fn dwell_time_blocks_voluntary_switching() {
        let config = TransportConfig::default();
        let (active, now) = measured(80, 0.05, 30, 5_000);

        let decision = should_switch(50.0, &active, Monotonic(2_000), 99.0, &config, now);

        assert_eq!(decision, SwitchDecision::Stay, "switched inside min_dwell");
    }

    #[test]
    fn unmeasured_paths_are_penalised_not_trusted() {
        let config = TransportConfig::default();
        let now = Monotonic(60_000);
        let unmeasured = PathMetrics::new(Monotonic::ZERO, 0);
        let (measured_path, _) = measured(15, 0.01, 8, 60_000);

        let unmeasured_score = score_path(&unmeasured, PathKind::Lan, &config, now);
        let measured_score = score_path(&measured_path, PathKind::Lan, &config, now);

        assert!(
            unmeasured_score < measured_score,
            "unmeasured {unmeasured_score} measured {measured_score}"
        );
    }

    #[test]
    fn scores_stay_in_range_at_the_extremes() {
        let config = TransportConfig::default();
        let now = Monotonic(120_000);

        let (perfect, _) = measured(1, 0.0, 0, 120_000);
        let (awful, _) = measured(2_000, 0.9, 500, 120_000);

        for kind in [PathKind::Lan, PathKind::WifiAware] {
            let p = score_path(&perfect, kind, &config, now);
            let a = score_path(&awful, kind, &config, now);
            assert!((0.0..=100.0).contains(&p), "{p}");
            assert!((0.0..=100.0).contains(&a), "{a}");
            assert!(p > a);
        }
    }

    #[test]
    fn loss_outweighs_latency() {
        // A fast lossy path should lose to a slower clean one: this is the
        // weighting decision in PathWeights::default(), asserted so that
        // changing it is a deliberate act.
        let config = TransportConfig::default();
        let (fast_lossy, now) = measured(4, 0.06, 3, 60_000);
        let (slow_clean, _) = measured(45, 0.0, 6, 60_000);

        let a = score_path(&fast_lossy, PathKind::Lan, &config, now);
        let b = score_path(&slow_clean, PathKind::Lan, &config, now);

        assert!(b > a, "fast+lossy {a} beat slow+clean {b}");
    }
}
