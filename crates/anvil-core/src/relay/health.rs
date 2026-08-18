//! Relay health monitoring (§40).
//!
//! Every participant watches the relay independently. Nobody is in charge of
//! deciding it has failed, because there is nobody to be in charge — and the
//! node most likely to notice first is the one the relay has stopped talking to.
//!
//! Detection is by missed heartbeats rather than by absence of media, because
//! VAD means a healthy room can be completely silent for a minute. A relay that
//! is quiet because nobody is speaking must not be mistaken for one that has
//! walked out of range.

use core::time::Duration;

use crate::time::Monotonic;
use crate::RelayConfig;

/// How healthy the relay looks from here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelayHealth {
    /// Responding normally.
    Healthy,
    /// Missing heartbeats but not yet written off. The UI can show a warning;
    /// the protocol does nothing yet.
    Degraded,
    /// Declared dead. Triggers an election (§41).
    Failed,
}

/// Watches one relay.
#[derive(Debug)]
pub struct RelayMonitor {
    last_heartbeat: Option<Monotonic>,
    interval: Duration,
    missed_limit: u32,
    /// Consecutive failures observed, for diagnostics.
    failures: u32,
}

impl RelayMonitor {
    /// Start monitoring, treating `now` as the last time the relay was seen.
    ///
    /// Starting from "seen now" rather than "never seen" avoids declaring a
    /// freshly elected relay dead before it has had a chance to send anything.
    #[must_use]
    pub fn new(config: &RelayConfig, heartbeat_interval: Duration, now: Monotonic) -> Self {
        Self {
            last_heartbeat: Some(now),
            interval: heartbeat_interval,
            missed_limit: config.missed_heartbeats,
            failures: 0,
        }
    }

    /// Record a heartbeat, or any traffic proving the relay is alive.
    pub fn on_heartbeat(&mut self, now: Monotonic) {
        self.last_heartbeat = Some(now);
    }

    /// Current health.
    #[must_use]
    pub fn health(&self, now: Monotonic) -> RelayHealth {
        let Some(last) = self.last_heartbeat else {
            return RelayHealth::Failed;
        };

        let silent = now.saturating_since(last);
        let missed = (silent.as_millis() / self.interval.as_millis().max(1)) as u32;

        if missed >= self.missed_limit {
            RelayHealth::Failed
        } else if missed >= 1 {
            RelayHealth::Degraded
        } else {
            RelayHealth::Healthy
        }
    }

    /// Whether an election should be triggered.
    #[must_use]
    pub fn has_failed(&self, now: Monotonic) -> bool {
        self.health(now) == RelayHealth::Failed
    }

    /// Note that this relay has failed, for the "keeps failing" signal.
    pub fn record_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
    }

    /// How many times this relay has been seen to fail.
    #[must_use]
    pub const fn failure_count(&self) -> u32 {
        self.failures
    }

    /// Reset after a new relay is elected.
    pub fn reset(&mut self, now: Monotonic) {
        self.last_heartbeat = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(now: Monotonic) -> RelayMonitor {
        RelayMonitor::new(&RelayConfig::default(), Duration::from_millis(500), now)
    }

    #[test]
    fn a_fresh_relay_is_healthy() {
        let m = monitor(Monotonic(1_000));
        assert_eq!(m.health(Monotonic(1_100)), RelayHealth::Healthy);
        assert!(!m.has_failed(Monotonic(1_100)));
    }

    #[test]
    fn missed_heartbeats_degrade_before_they_fail() {
        let m = monitor(Monotonic(0));

        assert_eq!(m.health(Monotonic(600)), RelayHealth::Degraded);
        assert_eq!(m.health(Monotonic(1_200)), RelayHealth::Degraded);
        // Three missed at 500ms each.
        assert_eq!(m.health(Monotonic(1_500)), RelayHealth::Failed);
    }

    #[test]
    fn a_heartbeat_restores_health() {
        let mut m = monitor(Monotonic(0));
        assert_eq!(m.health(Monotonic(1_200)), RelayHealth::Degraded);

        m.on_heartbeat(Monotonic(1_300));
        assert_eq!(m.health(Monotonic(1_400)), RelayHealth::Healthy);
    }

    #[test]
    fn a_silent_room_does_not_look_like_a_dead_relay() {
        // Nobody speaks for thirty seconds; heartbeats keep arriving.
        let mut m = monitor(Monotonic(0));
        for t in (0..30_000).step_by(500) {
            m.on_heartbeat(Monotonic(t));
            assert_ne!(m.health(Monotonic(t + 100)), RelayHealth::Failed);
        }
    }

    #[test]
    fn resetting_after_an_election_clears_the_previous_relays_silence() {
        let mut m = monitor(Monotonic(0));
        assert!(m.has_failed(Monotonic(10_000)));

        m.reset(Monotonic(10_000));
        assert_eq!(m.health(Monotonic(10_100)), RelayHealth::Healthy);
    }

    #[test]
    fn repeated_failures_are_counted() {
        let mut m = monitor(Monotonic(0));
        assert_eq!(m.failure_count(), 0);
        m.record_failure();
        m.record_failure();
        assert_eq!(m.failure_count(), 2);
    }
}
