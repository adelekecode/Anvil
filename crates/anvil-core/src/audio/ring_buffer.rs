//! Bounded PCM ring buffer for crossing the real-time audio boundary.
//!
//! The CPAL (or platform-native) input/output callback runs on an OS audio
//! thread with hard real-time constraints: no allocation, no locking that can
//! block, no I/O. This buffer is the safe handoff point.
//!
//! ```text
//!   capture callback   →  ring (write)  →  worker thread (read)
//!   worker thread (write)  →  ring (read)  →  playback callback
//! ```
//!
//! ## Policy
//!
//! * **Overflow** (producer outruns consumer): the oldest unread samples are
//!   silently dropped. On the capture side this means a stalled encoder loses
//!   the oldest audio, which is the right trade-off — old audio is useless.
//! * **Underflow** (consumer outruns producer): the reader gets silence. On
//!   the playback side this means the speaker callback plays zeroes until
//!   the pipeline catches up, which is a glitch the user hears but not a
//!   crash.
//!
//! Both directions are lock-free: one atomic for the write cursor, one for
//! the read cursor, with only one thread writing each.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Capacity of the ring buffer in interleaved `i16` samples.
///
/// Sized for ~100 ms of 48 kHz mono audio: 4,800 samples. Doubled because
/// the stereo-to-mono downmix happens before encoding and a stereo device
/// can briefly deliver twice the sample count.
pub const DEFAULT_CAPACITY: usize = 9_600;

/// A bounded SPSC PCM ring buffer for crossing the audio thread boundary.
///
/// The cursor indices are atomics for ordering; the buffer memory sits under
/// a `Mutex` that is only held during the copy-in / copy-out, which keeps
/// contention negligible — the two threads naturally produce and consume at
/// roughly the same rate.
///
/// The caller must guarantee that at most one thread calls [`Self::write`]
/// and at most one (different) thread calls [`Self::read`].
pub struct PcmRingBuffer {
    buf: Mutex<Box<[i16]>>,
    /// The producer's next write index (monotonically increasing; masked
    /// with capacity to index into `buf`).
    write: AtomicUsize,
    /// The consumer's next read index.
    read: AtomicUsize,
    /// Number of samples the buffer can hold. Always a power of two so the
    /// mask `capacity - 1` is a cheap modulo.
    capacity: usize,
    mask: usize,
}

impl PcmRingBuffer {
    /// Allocate a ring buffer. Capacity is rounded up to the next power of
    /// two for the bit-mask optimisation.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let pow2 = capacity.next_power_of_two();
        let buf = vec![0i16; pow2].into_boxed_slice();
        Self {
            buf: Mutex::new(buf),
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
            capacity: pow2,
            mask: pow2 - 1,
        }
    }

    /// Number of unread samples currently buffered.
    #[must_use]
    pub fn available(&self) -> usize {
        let w = self.write.load(Ordering::Acquire);
        let r = self.read.load(Ordering::Acquire);
        w.saturating_sub(r).min(self.capacity)
    }

    /// Space remaining before the buffer is full.
    #[must_use]
    pub fn remaining(&self) -> usize {
        self.capacity.saturating_sub(self.available())
    }

    /// Push interleaved samples from the producer side.
    ///
    /// If the buffer cannot accept everything, the oldest buffered samples
    /// are dropped to make room. Returns the number of samples that were
    /// lost (0 if there was enough room).
    ///
    /// This is the capture-callback path — must never block.
    pub fn write(&self, samples: &[i16]) -> usize {
        let len = samples.len();
        if len == 0 {
            return 0;
        }
        let avail = self.available();
        let cap = self.capacity;
        let mut dropped = 0;

        let w = self.write.load(Ordering::Relaxed);
        let r = self.read.load(Ordering::Acquire);

        // If we would overflow, advance the read cursor to drop the oldest
        // samples. How many do we need to drop?
        let needed = len.saturating_sub(cap.saturating_sub(avail));
        if needed > 0 {
            dropped = needed;
            // Advance read cursor past the oldest samples we are
            // discarding. The consumer will see the jump and skip them.
            self.read.store(r.wrapping_add(needed), Ordering::Release);
        }

        // Now there must be room. Write.
        let r2 = self.read.load(Ordering::Acquire);
        let space = cap.saturating_sub(w.saturating_sub(r2));
        let to_write = len.min(space);

        let start = w & self.mask;
        let end = (start + to_write) & self.mask;

        let mut buf = self.buf.lock().expect("ring buffer mutex poisoned");
        if start < end {
            buf[start..end].copy_from_slice(&samples[..to_write]);
        } else {
            // Wrap-around.
            let first_chunk = cap - start;
            buf[start..].copy_from_slice(&samples[..first_chunk]);
            buf[..end].copy_from_slice(&samples[first_chunk..to_write]);
        }

        self.write.store(w.wrapping_add(to_write), Ordering::Release);
        dropped
    }

    /// Pull samples for the consumer side.
    ///
    /// Returns the number of samples actually read, which may be fewer than
    /// `buf.len()` if the buffer is drained. The unfilled portion of `buf`
    /// is zeroed so the caller always receives a full frame — this is the
    /// playback-callback path where silence is the safe fallback.
    pub fn read(&self, buf: &mut [i16]) -> usize {
        let len = buf.len();
        if len == 0 {
            return 0;
        }
        let avail = self.available();
        let to_read = len.min(avail);

        let r = self.read.load(Ordering::Relaxed);
        let start = r & self.mask;
        let end = (start + to_read) & self.mask;

        let ring = self.buf.lock().expect("ring buffer mutex poisoned");
        if start < end {
            buf[..to_read].copy_from_slice(&ring[start..end]);
        } else if to_read > 0 {
            let first_chunk = self.capacity - start;
            buf[..first_chunk].copy_from_slice(&ring[start..]);
            buf[first_chunk..to_read].copy_from_slice(&ring[..end]);
        }

        // Zero the caller's remainder if we couldn't fill the whole buffer.
        // Playback underrun: the speaker callback needs *something*.
        if to_read < len {
            buf[to_read..].fill(0);
        }

        self.read
            .store(r.wrapping_add(to_read), Ordering::Release);
        to_read
    }

    /// Capacity in samples.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Reset the buffer to empty.
    pub fn clear(&self) {
        let w = self.write.load(Ordering::Relaxed);
        self.read.store(w, Ordering::Release);
    }
}

impl core::fmt::Debug for PcmRingBuffer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PcmRingBuffer")
            .field("capacity", &self.capacity)
            .field("available", &self.available())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn empty_buffer_has_no_samples() {
        let ring = PcmRingBuffer::new(1024);
        assert_eq!(ring.available(), 0);
    }

    #[test]
    fn write_read_roundtrips() {
        let ring = PcmRingBuffer::new(1024);
        let samples: Vec<i16> = (0..100).map(|i| i as i16).collect();

        ring.write(&samples);
        assert_eq!(ring.available(), 100);

        let mut out = vec![0i16; 100];
        assert_eq!(ring.read(&mut out), 100);
        assert_eq!(out, samples);
        assert_eq!(ring.available(), 0);
    }

    #[test]
    fn partial_read_returns_requested_count() {
        let ring = PcmRingBuffer::new(1024);
        ring.write(&[1, 2, 3, 4, 5]);

        let mut out = [0i16; 3];
        assert_eq!(ring.read(&mut out), 3);
        assert_eq!(out, [1, 2, 3]);
        assert_eq!(ring.available(), 2);
    }

    #[test]
    fn underflow_fills_with_silence() {
        let ring = PcmRingBuffer::new(1024);
        ring.write(&[1, 2]);

        let mut out = [0i16; 5];
        assert_eq!(ring.read(&mut out), 2);
        assert_eq!(out[0], 1);
        assert_eq!(out[1], 2);
        assert_eq!(out[2], 0);
        assert_eq!(out[3], 0);
        assert_eq!(out[4], 0);
    }

    #[test]
    fn overflow_drops_oldest_samples() {
        let ring = PcmRingBuffer::new(4);

        // Fill. Capacity is rounded up to next power of two (4).
        ring.write(&[1, 2, 3, 4]);
        assert_eq!(ring.available(), 4);

        // Overflow: one more sample drops the oldest.
        let dropped = ring.write(&[5]);
        assert_eq!(dropped, 1);

        let mut out = [0i16; 4];
        assert_eq!(ring.read(&mut out), 4);
        assert_eq!(out, [2, 3, 4, 5]);
    }

    #[test]
    fn wrap_around_handles_circular_write() {
        // Capacity = 8, fill 7, read 3, fill 6 → wrap.
        let ring = PcmRingBuffer::new(8);
        ring.write(&[1, 2, 3, 4, 5, 6, 7]);
        let mut out = [0i16; 3];
        ring.read(&mut out);

        ring.write(&[8, 9, 10, 11, 12, 13]);

        let mut out2 = [0i16; 10];
        assert_eq!(ring.read(&mut out2), 10);
        assert_eq!(out2[0], 4);
        assert_eq!(out2[9], 13);
    }

    #[test]
    fn clear_empties_buffer() {
        let ring = PcmRingBuffer::new(64);
        ring.write(&[1, 2, 3]);
        ring.clear();
        assert_eq!(ring.available(), 0);

        let mut out = [0i16; 3];
        assert_eq!(ring.read(&mut out), 0);
        assert_eq!(out, [0, 0, 0]);
    }

    #[test]
    fn large_overflow_drops_exactly_the_right_count() {
        let ring = PcmRingBuffer::new(8);
        ring.write(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let dropped = ring.write(&[9, 10, 11, 12, 13]); // 5 new, capacity=8
        assert!(dropped > 0, "should have dropped some samples");

        let mut out = vec![0i16; 8];
        ring.read(&mut out);
        // The last 8 samples written should survive.
        let first = out.iter().position(|s| *s != 0).unwrap_or(0);
        assert!(out[first] >= 6, "oldest sample should be 6 or later, got {}", out[first]);
        assert!(out.iter().any(|s| *s == 13), "newest sample should be present");
    }

    #[test]
    fn concurrent_single_producer_single_consumer() {
        let ring = std::sync::Arc::new(PcmRingBuffer::new(65536));
        let ring_tx = ring.clone();

        let producer = thread::spawn(move || {
            for i in 0..1_000u16 {
                ring_tx.write(&[i as i16; 960]);
            }
        });

        let consumer = thread::spawn(move || {
            let mut total = 0usize;
            let mut buf = vec![0i16; 960];
            while total < 960_000 {
                let n = ring.read(&mut buf);
                total += n;
                // Don't busy-wait; let the producer run.
                if n == 0 {
                    thread::yield_now();
                }
            }
            total
        });

        producer.join().unwrap();
        let total = consumer.join().unwrap();
        assert!(total >= 960_000 - 960, "consumer missed too many samples: {total}");
    }
}
