//! Adaptive jitter buffer (§29, §30).
//!
//! Packets do not arrive evenly over wireless. The buffer trades latency for
//! smoothness, and the whole design question is *how much*.
//!
//! ```text
//!   51 ──────┐
//!   53 ──┐   │
//!   52 ──┼───┼──►  [ 51 52 53 ]  ──►  decoder
//!        ▼   ▼
//!      reordered, spaced
//! ```
//!
//! ## Adaptation
//!
//! Target depth tracks observed arrival jitter, clamped to
//! [`crate::AudioConfig::jitter_min`]..`jitter_max`. It grows **fast** and
//! shrinks **slowly**, which is the asymmetry that matters: under-buffering is
//! immediately audible as choppy audio, while over-buffering costs a little
//! conversational latency nobody notices at these magnitudes. When in doubt,
//! buffer.
//!
//! ## Loss
//!
//! A gap is not a failure (§30). Missing frames are reported to the caller so
//! Opus packet-loss concealment can synthesise a plausible replacement, and the
//! stream continues. A buffer that stalled waiting for a frame that is never
//! coming would turn a 20 ms artefact into a broken call.
//!
//! ## Talkspurts
//!
//! After VAD silence, the next frame may arrive at any offset. Playout timing
//! resets on a talkspurt boundary rather than treating the silent period as an
//! enormous burst of loss.

use core::time::Duration;
use std::collections::BTreeMap;

use crate::audio::frame::EncodedFrame;
use crate::time::Monotonic;
use crate::{AudioConfig, SeqNum};

/// Growth and shrink rates for the target depth.
const GROW: f32 = 0.5;
const SHRINK: f32 = 0.01;

/// What came out of the buffer for one playout slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Playout {
    /// A frame is ready.
    Frame(EncodedFrame),
    /// The expected frame never arrived; conceal it.
    Concealed {
        /// Which sequence number was missing.
        sequence: SeqNum,
    },
    /// Nothing is buffered — the stream is silent or has not started. Play
    /// silence; this is not loss and must not be counted as such.
    Silent,
}

/// Per-stream jitter buffer.
#[derive(Debug)]
pub struct JitterBuffer {
    frames: BTreeMap<u32, EncodedFrame>,
    /// Next sequence number to play.
    next: Option<SeqNum>,
    /// Current target depth.
    target: Duration,
    min: Duration,
    max: Duration,
    /// Smoothed arrival jitter estimate.
    jitter_estimate: f32,
    last_arrival: Option<Monotonic>,
    /// Frames concealed since the stream started.
    concealed: u64,
    /// Frames dropped for arriving after their playout slot.
    late: u64,
    /// Frames played.
    played: u64,
    frame_duration: Duration,
}

impl JitterBuffer {
    /// Build a buffer from config.
    #[must_use]
    pub fn new(config: &AudioConfig) -> Self {
        Self {
            frames: BTreeMap::new(),
            next: None,
            target: config.jitter_initial,
            min: config.jitter_min,
            max: config.jitter_max,
            // The estimate is a *jitter* figure, not a buffer depth. Seed it so
            // that the target it produces equals `jitter_initial` — seeding it
            // with the depth itself would start the buffer at roughly twice the
            // configured latency and take many seconds to come back down.
            jitter_estimate: config
                .jitter_initial
                .saturating_sub(config.frame_duration)
                .as_secs_f32()
                * 500.0,
            last_arrival: None,
            concealed: 0,
            late: 0,
            played: 0,
            frame_duration: config.frame_duration,
        }
    }

    /// Insert an arriving frame.
    ///
    /// Returns false if the frame was dropped for arriving after its playout
    /// slot had already passed — nothing useful can be done with it, and
    /// inserting it would play audio out of order.
    pub fn push(&mut self, frame: EncodedFrame, now: Monotonic) -> bool {
        self.observe_arrival(now);

        if frame.talkspurt_start {
            // New talkspurt: resynchronise rather than treating the silence as
            // a huge run of loss.
            self.frames.clear();
            self.next = Some(frame.sequence);
        }

        if let Some(next) = self.next {
            if next.is_newer_than(frame.sequence) {
                self.late += 1;
                return false;
            }
        } else {
            self.next = Some(frame.sequence);
        }

        self.frames.insert(frame.sequence.0, frame);
        true
    }

    /// Take the next frame for playout.
    ///
    /// Called on the audio clock, once per frame duration.
    pub fn pop(&mut self) -> Playout {
        let Some(next) = self.next else {
            return Playout::Silent;
        };

        // Hold until the buffer has reached its target depth, so playout starts
        // with enough cushion to absorb the jitter already measured.
        if self.buffered_duration() < self.target && !self.frames.contains_key(&next.0) {
            return Playout::Silent;
        }

        if let Some(frame) = self.frames.remove(&next.0) {
            self.next = Some(next.next());
            self.played += 1;
            return Playout::Frame(frame);
        }

        // Missing. If something later is buffered, the frame is genuinely lost;
        // conceal it and move on rather than stalling the stream.
        if self.frames.keys().any(|seq| SeqNum(*seq).is_newer_than(next)) {
            self.next = Some(next.next());
            self.concealed += 1;
            return Playout::Concealed { sequence: next };
        }

        Playout::Silent
    }

    /// Current buffer occupancy in time.
    #[must_use]
    pub fn buffered_duration(&self) -> Duration {
        self.frame_duration * self.frames.len() as u32
    }

    /// Current target depth.
    #[must_use]
    pub const fn target_depth(&self) -> Duration {
        self.target
    }

    /// Frames concealed so far.
    #[must_use]
    pub const fn concealed_count(&self) -> u64 {
        self.concealed
    }

    /// Frames dropped for arriving too late.
    #[must_use]
    pub const fn late_count(&self) -> u64 {
        self.late
    }

    /// Frames played.
    #[must_use]
    pub const fn played_count(&self) -> u64 {
        self.played
    }

    /// Reset for a new stream or after a path change that invalidates timing.
    pub fn reset(&mut self) {
        self.frames.clear();
        self.next = None;
        self.last_arrival = None;
    }

    fn observe_arrival(&mut self, now: Monotonic) {
        let Some(last) = self.last_arrival else {
            self.last_arrival = Some(now);
            return;
        };
        self.last_arrival = Some(now);

        // Deviation of actual spacing from the expected frame cadence.
        let gap_ms = now.saturating_since(last).as_secs_f32() * 1000.0;
        let expected_ms = self.frame_duration.as_secs_f32() * 1000.0;
        let deviation = (gap_ms - expected_ms).abs();

        // Fast up, slow down.
        let rate = if deviation > self.jitter_estimate { GROW } else { SHRINK };
        self.jitter_estimate += rate * (deviation - self.jitter_estimate);

        // Two jitter estimates of headroom, plus one frame.
        let target_ms = self.jitter_estimate * 2.0 + expected_ms;
        self.target = Duration::from_millis(target_ms as u64).clamp(self.min, self.max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MediaTimestamp;

    fn frame(seq: u32) -> EncodedFrame {
        EncodedFrame {
            payload: vec![seq as u8; 60],
            sequence: SeqNum(seq),
            timestamp: MediaTimestamp(seq * 960),
            talkspurt_start: false,
        }
    }

    fn talkspurt(seq: u32) -> EncodedFrame {
        EncodedFrame { talkspurt_start: true, ..frame(seq) }
    }

    /// Fill past the target depth so playout can begin.
    fn prime(buffer: &mut JitterBuffer, from: u32, count: u32) {
        for i in 0..count {
            buffer.push(frame(from + i), Monotonic(u64::from(i) * 20));
        }
    }

    #[test]
    fn plays_frames_in_order() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        prime(&mut buffer, 0, 10);

        for expected in 0..5 {
            match buffer.pop() {
                Playout::Frame(f) => assert_eq!(f.sequence, SeqNum(expected)),
                other => panic!("expected frame {expected}, got {other:?}"),
            }
        }
    }

    #[test]
    fn reorders_packets_that_arrive_out_of_order() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        // 51, 53, 52 — the §29 example.
        buffer.push(frame(51), Monotonic(0));
        buffer.push(frame(53), Monotonic(20));
        buffer.push(frame(52), Monotonic(25));
        for i in 54..62 {
            buffer.push(frame(i), Monotonic(u64::from(i) * 20));
        }

        let played: Vec<u32> = (0..3)
            .map(|_| match buffer.pop() {
                Playout::Frame(f) => f.sequence.0,
                other => panic!("{other:?}"),
            })
            .collect();

        assert_eq!(played, vec![51, 52, 53]);
    }

    #[test]
    fn a_lost_frame_is_concealed_rather_than_stalling_the_stream() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        for seq in [100, 101, 103, 104, 105, 106, 107, 108] {
            buffer.push(frame(seq), Monotonic(u64::from(seq) * 20));
        }

        assert!(matches!(buffer.pop(), Playout::Frame(f) if f.sequence == SeqNum(100)));
        assert!(matches!(buffer.pop(), Playout::Frame(f) if f.sequence == SeqNum(101)));
        assert_eq!(buffer.pop(), Playout::Concealed { sequence: SeqNum(102) });
        assert!(matches!(buffer.pop(), Playout::Frame(f) if f.sequence == SeqNum(103)));
        assert_eq!(buffer.concealed_count(), 1);
    }

    #[test]
    fn frames_arriving_after_their_slot_are_dropped_not_played_late() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        prime(&mut buffer, 200, 8);
        for _ in 0..4 {
            buffer.pop();
        }

        assert!(!buffer.push(frame(200), Monotonic(500)), "played a frame out of order");
        assert_eq!(buffer.late_count(), 1);
    }

    #[test]
    fn an_empty_buffer_is_silent_not_lossy() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        assert_eq!(buffer.pop(), Playout::Silent);
        assert_eq!(buffer.concealed_count(), 0, "silence was counted as loss");
    }

    #[test]
    fn a_talkspurt_resynchronises_after_vad_silence() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        prime(&mut buffer, 0, 8);
        buffer.pop();

        // Long VAD gap, then speech resumes at a far higher sequence number.
        buffer.push(talkspurt(9_000), Monotonic(30_000));
        for i in 1..8 {
            buffer.push(frame(9_000 + i), Monotonic(30_000 + u64::from(i) * 20));
        }

        // Must not conceal the ~8,990 frames that were never sent.
        let concealed_before = buffer.concealed_count();
        assert!(matches!(buffer.pop(), Playout::Frame(f) if f.sequence == SeqNum(9_000)));
        assert_eq!(buffer.concealed_count(), concealed_before);
    }

    #[test]
    fn target_depth_grows_under_jitter_and_stays_within_bounds() {
        let config = AudioConfig::default();
        let mut buffer = JitterBuffer::new(&config);

        // Wildly uneven arrivals.
        let arrivals = [0u64, 5, 90, 95, 100, 220, 225, 230, 400];
        for (i, t) in arrivals.iter().enumerate() {
            buffer.push(frame(i as u32), Monotonic(*t));
        }

        assert!(buffer.target_depth() > config.jitter_min);
        assert!(buffer.target_depth() <= config.jitter_max, "exceeded the latency ceiling");
    }

    #[test]
    fn target_depth_never_exceeds_the_configured_ceiling() {
        let config = AudioConfig::default();
        let mut buffer = JitterBuffer::new(&config);

        // Pathological: seconds between packets.
        for i in 0..20u64 {
            buffer.push(frame(i as u32), Monotonic(i * 3_000));
        }
        assert!(buffer.target_depth() <= config.jitter_max);
    }

    #[test]
    fn reset_clears_everything() {
        let mut buffer = JitterBuffer::new(&AudioConfig::default());
        prime(&mut buffer, 0, 10);
        buffer.reset();
        assert_eq!(buffer.pop(), Playout::Silent);
        assert_eq!(buffer.buffered_duration(), Duration::ZERO);
    }
}
