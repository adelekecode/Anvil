//! Per-path measurement (§17, §83).
//!
//! Deliberately cheap. §83 is explicit that path monitoring must not mean
//! continuous benchmarking — the metrics here are derived from traffic Anvil
//! was going to send anyway (media, heartbeats, control), with active probes
//! reserved for a standby path that is otherwise silent.
//!
//! Every value is an exponentially weighted moving average. EWMAs are used
//! rather than windowed averages because they need one f32 of state per metric,
//! which matters when a room has several peers each with two paths, and because
//! their "forget the past smoothly" behaviour is exactly what path scoring
//! wants — a burst of loss thirty seconds ago should not still be voting.

use core::time::Duration;

use crate::time::Monotonic;

/// Smoothing factor for RTT and jitter. Lower reacts faster.
const RTT_ALPHA: f32 = 0.125; // same as TCP's, for the same reasons
const JITTER_ALPHA: f32 = 0.25;
const LOSS_ALPHA: f32 = 0.10; // loss is noisy; smooth it harder

/// One observation about a path.
#[derive(Clone, Copy, Debug)]
pub enum PathSample {
    /// A round trip completed.
    Rtt(Duration),
    /// A packet arrived. `transit_delta` is the difference between inter-arrival
    /// spacing and inter-send spacing — the RFC 3550 jitter input.
    Arrival {
        /// Signed spacing difference, in milliseconds.
        transit_delta_ms: f32,
    },
    /// A run of packets was accounted for.
    Delivery {
        /// How many were expected.
        expected: u32,
        /// How many arrived.
        received: u32,
    },
    /// The path failed and was re-established. Resets accumulated stability.
    Disruption,
}

/// Smoothed health of one path.
#[derive(Clone, Copy, Debug)]
pub struct PathMetrics {
    /// Smoothed round-trip time.
    pub rtt: Duration,
    /// Smoothed arrival jitter.
    pub jitter: Duration,
    /// Smoothed loss fraction, 0.0–1.0.
    pub loss: f32,
    /// When the path last became usable.
    pub established_at: Monotonic,
    /// Last time anything at all arrived. Drives the hard-failure timeout.
    pub last_activity: Monotonic,
    /// Disruptions since this path was first created.
    pub disruptions: u32,
    /// Whether any measurement has been taken yet. An unmeasured path scores
    /// pessimistically rather than optimistically — assuming an untested path
    /// is good is how you switch a live call onto a path that does not work.
    pub measured: bool,
    /// Forwarding hops. 0 = direct, 1 = via relay.
    pub hops: u8,
}

impl PathMetrics {
    /// A fresh, unmeasured path.
    #[must_use]
    pub fn new(now: Monotonic, hops: u8) -> Self {
        Self {
            rtt: Duration::from_millis(100),
            jitter: Duration::from_millis(20),
            loss: 0.0,
            established_at: now,
            last_activity: now,
            disruptions: 0,
            measured: false,
            hops,
        }
    }

    /// Fold in an observation.
    pub fn observe(&mut self, sample: PathSample, now: Monotonic) {
        self.last_activity = now;

        match sample {
            PathSample::Rtt(rtt) => {
                self.rtt = ewma_duration(self.rtt, rtt, RTT_ALPHA, self.measured);
                self.measured = true;
            }
            PathSample::Arrival { transit_delta_ms } => {
                let observed = Duration::from_micros((transit_delta_ms.abs() * 1000.0) as u64);
                self.jitter = ewma_duration(self.jitter, observed, JITTER_ALPHA, self.measured);
                self.measured = true;
            }
            PathSample::Delivery { expected, received } => {
                if expected > 0 {
                    let lost = expected.saturating_sub(received) as f32 / expected as f32;
                    self.loss = ewma(self.loss, lost, LOSS_ALPHA, self.measured);
                    self.measured = true;
                }
            }
            PathSample::Disruption => {
                self.disruptions = self.disruptions.saturating_add(1);
                self.established_at = now;
                // Do not reset rtt/loss: the path's recent behaviour is still
                // the best evidence we have about what it will do next.
            }
        }
    }

    /// How long the path has been up without a disruption.
    #[must_use]
    pub fn uptime(&self, now: Monotonic) -> Duration {
        now.saturating_since(self.established_at)
    }

    /// Time since anything arrived.
    #[must_use]
    pub fn idle_for(&self, now: Monotonic) -> Duration {
        now.saturating_since(self.last_activity)
    }

    /// Whether the path has gone quiet past its timeout, which counts as hard
    /// failure and bypasses hysteresis (§85).
    #[must_use]
    pub fn is_stale(&self, now: Monotonic, timeout: Duration) -> bool {
        self.idle_for(now) > timeout
    }
}

fn ewma(current: f32, sample: f32, alpha: f32, initialised: bool) -> f32 {
    if initialised {
        current + alpha * (sample - current)
    } else {
        // First real measurement replaces the placeholder outright. Blending
        // against a made-up default just makes the first few seconds lie.
        sample
    }
}

fn ewma_duration(current: Duration, sample: Duration, alpha: f32, initialised: bool) -> Duration {
    let millis =
        ewma(current.as_secs_f32() * 1000.0, sample.as_secs_f32() * 1000.0, alpha, initialised);
    Duration::from_micros((millis.max(0.0) * 1000.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_measurement_replaces_the_placeholder() {
        let mut m = PathMetrics::new(Monotonic::ZERO, 0);
        assert!(!m.measured);

        m.observe(PathSample::Rtt(Duration::from_millis(4)), Monotonic(10));

        assert!(m.measured);
        // Not blended with the 100ms default.
        assert!(m.rtt < Duration::from_millis(5), "rtt was {:?}", m.rtt);
    }

    #[test]
    fn subsequent_measurements_are_smoothed_not_replaced() {
        let mut m = PathMetrics::new(Monotonic::ZERO, 0);
        m.observe(PathSample::Rtt(Duration::from_millis(10)), Monotonic(10));
        m.observe(PathSample::Rtt(Duration::from_millis(200)), Monotonic(20));

        // One outlier must not dominate, or scoring flaps on a single spike.
        assert!(m.rtt < Duration::from_millis(40), "rtt was {:?}", m.rtt);
        assert!(m.rtt > Duration::from_millis(10));
    }

    #[test]
    fn loss_is_tracked_as_a_fraction() {
        let mut m = PathMetrics::new(Monotonic::ZERO, 0);
        m.observe(PathSample::Delivery { expected: 100, received: 95 }, Monotonic(10));
        assert!((m.loss - 0.05).abs() < 0.001, "loss was {}", m.loss);
    }

    #[test]
    fn disruption_resets_uptime_but_keeps_history() {
        let mut m = PathMetrics::new(Monotonic::ZERO, 0);
        m.observe(PathSample::Delivery { expected: 100, received: 90 }, Monotonic(1_000));
        let loss_before = m.loss;

        m.observe(PathSample::Disruption, Monotonic(5_000));

        assert_eq!(m.disruptions, 1);
        assert_eq!(m.uptime(Monotonic(5_000)), Duration::ZERO);
        assert_eq!(m.loss, loss_before);
    }

    #[test]
    fn staleness_is_measured_from_last_activity() {
        let mut m = PathMetrics::new(Monotonic::ZERO, 0);
        m.observe(PathSample::Rtt(Duration::from_millis(5)), Monotonic(1_000));

        assert!(!m.is_stale(Monotonic(3_000), Duration::from_secs(3)));
        assert!(m.is_stale(Monotonic(5_000), Duration::from_secs(3)));
    }
}
