//! Voice activity detection (§28).
//!
//! Not sending anything while nobody is speaking saves bandwidth, radio airtime,
//! relay load and battery — in a four-person room where one person is talking,
//! it removes three quarters of the media traffic.
//!
//! It is also the easiest way to make a voice product feel broken. Get the
//! thresholds wrong and you clip the first syllable of every sentence, or cut
//! off the end of words, or gate out someone with a quiet voice. Both failure
//! modes are worse than the bandwidth they save.
//!
//! Two defences against that:
//!
//! * **Hangover.** Transmission continues for
//!   [`crate::AudioConfig::vad_hangover`] after speech stops, so brief pauses
//!   between words and trailing consonants (which carry very little energy but
//!   a great deal of intelligibility) survive.
//! * **An adaptive noise floor.** The threshold tracks the room, so a noisy
//!   café and a silent bedroom both work. It adapts *upward slowly and downward
//!   quickly*, which biases the detector toward sending — the safe direction.
//!
//! This is an energy-based detector: cheap, predictable and good enough when
//! the alternative failure is a dropped word. Something spectral could be
//! considered during Phase 7 tuning if measurements justify it.

use core::time::Duration;

use crate::time::Monotonic;

/// Speech is declared when frame energy exceeds the noise floor by this factor.
const SPEECH_MARGIN: f32 = 3.0;

/// Noise floor adaptation rates, all asymmetric on purpose.
///
/// The floor falls quickly (a room that goes quiet should be usable
/// immediately) and rises slowly (a loud voice must not raise the floor above
/// itself and gate the speaker out mid-sentence).
///
/// [`FLOOR_RISE_DURING_SPEECH`] is the subtle one. Adapting *only* on frames
/// classified as non-speech sounds right, but it deadlocks: in a room with
/// steady loud noise — a fan, a generator, a car — every frame classifies as
/// speech, the floor never learns the noise, and the detector transmits
/// continuously forever. A very slow rise during speech breaks that, over tens
/// of seconds, without meaningfully affecting someone who is actually talking.
const FLOOR_RISE: f32 = 0.005;
const FLOOR_RISE_DURING_SPEECH: f32 = 0.001;
const FLOOR_FALL: f32 = 0.05;

/// Absolute floor, so a perfectly silent input does not make the threshold zero
/// and classify DC noise as speech.
const MIN_FLOOR: f32 = 30.0;

/// Energy-based voice activity detector with hangover.
#[derive(Clone, Debug)]
pub struct VoiceActivityDetector {
    noise_floor: f32,
    speaking: bool,
    last_speech: Monotonic,
    hangover: Duration,
    enabled: bool,
}

impl VoiceActivityDetector {
    /// Build a detector from config.
    #[must_use]
    pub fn new(config: &crate::AudioConfig) -> Self {
        Self {
            noise_floor: MIN_FLOOR,
            speaking: false,
            last_speech: Monotonic::ZERO,
            hangover: config.vad_hangover,
            enabled: config.vad_enabled,
        }
    }

    /// Whether this frame should be transmitted.
    ///
    /// Returns true during hangover even when the frame itself is silent.
    pub fn should_transmit(&mut self, samples: &[i16], now: Monotonic) -> bool {
        if !self.enabled {
            return true;
        }

        let energy = rms(samples);
        let is_speech = energy > self.noise_floor * SPEECH_MARGIN;

        let rate = if is_speech {
            FLOOR_RISE_DURING_SPEECH
        } else if energy < self.noise_floor {
            FLOOR_FALL
        } else {
            FLOOR_RISE
        };
        self.noise_floor += rate * (energy - self.noise_floor);
        self.noise_floor = self.noise_floor.max(MIN_FLOOR);

        if is_speech {
            self.speaking = true;
            self.last_speech = now;
            return true;
        }

        if self.speaking && now.saturating_since(self.last_speech) <= self.hangover {
            return true; // hangover: still sending
        }

        self.speaking = false;
        false
    }

    /// Whether the detector currently considers the user to be speaking.
    ///
    /// Drives [`crate::Event::SpeakingChanged`] and the talking indicator.
    #[must_use]
    pub const fn is_speaking(&self) -> bool {
        self.speaking
    }

    /// Current adaptive noise floor, for diagnostics.
    #[must_use]
    pub const fn noise_floor(&self) -> f32 {
        self.noise_floor
    }
}

/// Root-mean-square amplitude of a frame.
#[must_use]
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AudioConfig;

    fn tone(amplitude: i16, len: usize) -> Vec<i16> {
        (0..len)
            .map(|i| {
                let phase = (i as f32) * 0.1;
                (phase.sin() * amplitude as f32) as i16
            })
            .collect()
    }

    #[test]
    fn silence_is_not_transmitted() {
        let config = AudioConfig::default();
        let mut vad = VoiceActivityDetector::new(&config);
        let silence = vec![0i16; 960];

        // Let the floor settle.
        for t in 0..20 {
            vad.should_transmit(&silence, Monotonic(t * 20));
        }
        assert!(!vad.should_transmit(&silence, Monotonic(1_000)));
        assert!(!vad.is_speaking());
    }

    #[test]
    fn speech_is_transmitted() {
        let config = AudioConfig::default();
        let mut vad = VoiceActivityDetector::new(&config);

        assert!(vad.should_transmit(&tone(8_000, 960), Monotonic(100)));
        assert!(vad.is_speaking());
    }

    #[test]
    fn hangover_keeps_the_tail_of_a_word() {
        let config = AudioConfig::default(); // 300ms hangover
        let mut vad = VoiceActivityDetector::new(&config);
        let silence = vec![0i16; 960];

        vad.should_transmit(&tone(8_000, 960), Monotonic(1_000));

        // Brief pause between words: still sending.
        assert!(vad.should_transmit(&silence, Monotonic(1_100)));
        assert!(vad.should_transmit(&silence, Monotonic(1_250)));

        // Genuinely finished: stop.
        assert!(!vad.should_transmit(&silence, Monotonic(1_400)));
    }

    #[test]
    fn a_quiet_voice_in_a_quiet_room_is_still_detected() {
        // The failure this guards against: a fixed threshold gating out someone
        // speaking softly.
        let config = AudioConfig::default();
        let mut vad = VoiceActivityDetector::new(&config);
        let silence = vec![0i16; 960];

        for t in 0..50 {
            vad.should_transmit(&silence, Monotonic(t * 20));
        }
        assert!(vad.should_transmit(&tone(600, 960), Monotonic(2_000)));
    }

    #[test]
    fn the_noise_floor_adapts_to_a_noisy_room() {
        let config = AudioConfig::default();
        let mut vad = VoiceActivityDetector::new(&config);
        let room_noise = tone(400, 960);

        let floor_before = vad.noise_floor();
        for t in 0..400 {
            vad.should_transmit(&room_noise, Monotonic(t * 20));
        }

        assert!(vad.noise_floor() > floor_before, "floor never adapted upward");
        // Sustained background noise alone must not read as speech forever.
        assert!(!vad.should_transmit(&room_noise, Monotonic(100_000)));
    }

    #[test]
    fn disabling_vad_transmits_everything() {
        let config = AudioConfig { vad_enabled: false, ..AudioConfig::default() };
        let mut vad = VoiceActivityDetector::new(&config);
        assert!(vad.should_transmit(&vec![0i16; 960], Monotonic(0)));
    }

    #[test]
    fn rms_of_silence_is_zero() {
        assert_eq!(rms(&[0, 0, 0, 0]), 0.0);
        assert_eq!(rms(&[]), 0.0);
        assert!(rms(&[i16::MAX; 16]) > 30_000.0);
    }
}
