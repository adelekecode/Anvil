//! Clocks.
//!
//! Anvil never uses wall-clock time for anything that matters. Phones disagree
//! about what time it is, adjust their clocks mid-call, and have no NTP source
//! when there is no Internet — which is Anvil's normal operating condition.
//!
//! So there are exactly two clocks:
//!
//! * [`Monotonic`] — local elapsed time, for timeouts, RTT and scoring. Only
//!   ever compared against other `Monotonic` values from the same device.
//! * [`MediaTimestamp`] — a per-stream sample counter set by the sender, used
//!   by the receiver's jitter buffer for playback spacing. Comparable only
//!   within one stream.

use core::fmt;
use core::ops::{Add, Sub};
use core::time::Duration;

/// A local monotonic instant, in milliseconds since node start.
///
/// Milliseconds are enough: the shortest interval Anvil reasons about is a
/// 10 ms Opus frame, and using a plain integer keeps this `Copy`, comparable
/// and trivially sendable across the FFI boundary for diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Monotonic(pub u64);

impl Monotonic {
    /// The node's zero point.
    pub const ZERO: Self = Self(0);

    /// Milliseconds since node start.
    #[must_use]
    pub const fn as_millis(self) -> u64 {
        self.0
    }

    /// Time elapsed since `earlier`. Saturates at zero rather than panicking —
    /// a negative interval means a bookkeeping bug, and a bookkeeping bug
    /// should not take down a live call.
    #[must_use]
    pub const fn saturating_since(self, earlier: Self) -> Duration {
        Duration::from_millis(self.0.saturating_sub(earlier.0))
    }
}

impl Add<Duration> for Monotonic {
    type Output = Self;
    fn add(self, rhs: Duration) -> Self {
        Self(self.0.saturating_add(rhs.as_millis() as u64))
    }
}

impl Sub<Monotonic> for Monotonic {
    type Output = Duration;
    fn sub(self, rhs: Self) -> Duration {
        self.saturating_since(rhs)
    }
}

impl fmt::Display for Monotonic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "t+{}ms", self.0)
    }
}

/// Sender-assigned media timestamp, in samples at the stream's clock rate.
///
/// Wraps, like [`crate::SeqNum`]. Two timestamps from *different* streams mean
/// nothing next to each other — the mixer aligns streams by arrival and
/// per-stream playout, not by comparing timestamps across senders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct MediaTimestamp(pub u32);

impl MediaTimestamp {
    /// Advance by one frame of `samples`.
    #[must_use]
    pub const fn advance(self, samples: u32) -> Self {
        Self(self.0.wrapping_add(samples))
    }

    /// Wrap-aware forward distance in samples, or `None` if `self` precedes
    /// `other`.
    #[must_use]
    pub const fn samples_since(self, other: Self) -> Option<u32> {
        let diff = self.0.wrapping_sub(other.0);
        if diff < (u32::MAX / 2) {
            Some(diff)
        } else {
            None
        }
    }
}

/// Source of monotonic time.
///
/// A trait rather than a direct `Instant::now()` call so that jitter buffer,
/// path scoring, election hysteresis and timeout logic can all be tested
/// deterministically. Every timing-sensitive decision in Anvil is a pure
/// function of values from this clock, which is what makes those tests possible.
pub trait Clock: Send + Sync + fmt::Debug {
    /// Current monotonic time.
    fn now(&self) -> Monotonic;
}

/// The real clock, backed by [`std::time::Instant`].
#[derive(Debug)]
pub struct SystemClock {
    origin: std::time::Instant,
}

impl SystemClock {
    /// Start a clock whose zero point is now.
    #[must_use]
    pub fn new() -> Self {
        Self { origin: std::time::Instant::now() }
    }
}

impl Default for SystemClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for SystemClock {
    fn now(&self) -> Monotonic {
        Monotonic(self.origin.elapsed().as_millis() as u64)
    }
}

/// A clock you drive by hand. Tests only.
#[derive(Debug, Default)]
pub struct TestClock {
    now: std::sync::atomic::AtomicU64,
}

impl TestClock {
    /// A clock sitting at zero.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Move time forward.
    pub fn advance(&self, by: Duration) {
        self.now.fetch_add(by.as_millis() as u64, std::sync::atomic::Ordering::SeqCst);
    }
}

impl Clock for TestClock {
    fn now(&self) -> Monotonic {
        Monotonic(self.now.load(std::sync::atomic::Ordering::SeqCst))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clock_advances() {
        let clock = TestClock::new();
        assert_eq!(clock.now(), Monotonic::ZERO);
        clock.advance(Duration::from_millis(250));
        assert_eq!(clock.now(), Monotonic(250));
    }

    #[test]
    fn backwards_intervals_saturate_instead_of_panicking() {
        let early = Monotonic(10);
        let late = Monotonic(100);
        assert_eq!(late - early, Duration::from_millis(90));
        assert_eq!(early - late, Duration::ZERO);
    }

    #[test]
    fn media_timestamps_survive_wrap() {
        let near_max = MediaTimestamp(u32::MAX - 100);
        let wrapped = near_max.advance(960); // 20ms @ 48kHz

        assert_eq!(wrapped.samples_since(near_max), Some(960));
        assert_eq!(near_max.samples_since(wrapped), None);
    }
}
