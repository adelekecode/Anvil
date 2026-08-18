//! Tunables.
//!
//! Everything the spec says should be "configurable and tuned through testing"
//! lives here rather than as constants scattered through the code. The defaults
//! below are starting points chosen to be *measured against*, not believed —
//! §93 is explicit that these numbers must come from real devices.

use core::time::Duration;

/// Top-level node configuration.
#[derive(Clone, Debug)]
pub struct AnvilConfig {
    /// Human-visible display name advertised during discovery.
    ///
    /// Unauthenticated until the handshake completes. The UI must not present
    /// a discovered name as trusted before then — anyone nearby can advertise
    /// any string.
    pub display_name: String,

    /// Audio pipeline settings.
    pub audio: AudioConfig,

    /// Transport selection settings.
    pub transport: TransportConfig,

    /// Relay election settings.
    pub relay: RelayConfig,

    /// Emit verbose diagnostics events (§92). Off by default: the event volume
    /// is high enough to matter on a phone.
    pub diagnostics: bool,
}

impl Default for AnvilConfig {
    fn default() -> Self {
        Self {
            display_name: String::from("Anvil device"),
            audio: AudioConfig::default(),
            transport: TransportConfig::default(),
            relay: RelayConfig::default(),
            diagnostics: false,
        }
    }
}

/// Audio pipeline configuration.
#[derive(Clone, Copy, Debug)]
pub struct AudioConfig {
    /// Sample rate. 48 kHz is Opus's native rate; anything else costs a
    /// resample on both ends for no benefit.
    pub sample_rate_hz: u32,

    /// Channels. Voice is mono — stereo doubles the bitrate to transmit
    /// information nobody can use on a phone speaker.
    pub channels: u8,

    /// Opus frame duration.
    ///
    /// 20 ms is the standard voice trade-off: 10 ms halves the frame delay but
    /// nearly doubles per-packet header overhead, which matters a great deal
    /// when a relay is fanning out to three peers over a shared radio.
    pub frame_duration: Duration,

    /// Target Opus bitrate in bits per second.
    ///
    /// 24 kbps is comfortably transparent for speech at 48 kHz mono. Raise it
    /// only if listening tests say so.
    pub target_bitrate_bps: u32,

    /// Ask Opus for in-band forward error correction.
    pub opus_fec: bool,

    /// Suppress transmission during detected silence (§28).
    pub vad_enabled: bool,

    /// Keep transmitting for this long after speech stops, so word-final
    /// consonants and short pauses are not clipped. This is the single most
    /// common way VAD ruins a voice product.
    pub vad_hangover: Duration,

    /// Jitter buffer floor.
    pub jitter_min: Duration,

    /// Jitter buffer ceiling. Past this, added delay hurts conversation more
    /// than the concealed loss it prevents.
    pub jitter_max: Duration,

    /// Jitter buffer starting depth before any measurements exist.
    pub jitter_initial: Duration,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            sample_rate_hz: 48_000,
            channels: 1,
            frame_duration: Duration::from_millis(20),
            target_bitrate_bps: 24_000,
            opus_fec: true,
            vad_enabled: true,
            vad_hangover: Duration::from_millis(300),
            jitter_min: Duration::from_millis(20),
            jitter_max: Duration::from_millis(200),
            jitter_initial: Duration::from_millis(60),
        }
    }
}

/// Transport selection and failover configuration (§17–§19, §84–§85).
#[derive(Clone, Copy, Debug)]
pub struct TransportConfig {
    /// Weights applied to each normalised path metric when scoring (§18).
    pub weights: PathWeights,

    /// Static preference applied when scores are close (§19), expressed as a
    /// bonus added to a path's score.
    ///
    /// Small on purpose: measured quality is supposed to win. This only breaks
    /// ties.
    pub lan_preference_bonus: f32,

    /// A candidate path must beat the active path by at least this much before
    /// a *voluntary* switch happens (§84).
    ///
    /// This is the anti-flapping knob. Too low and the call ping-pongs between
    /// radios; too high and a genuinely better path never gets used.
    pub switch_hysteresis: f32,

    /// Minimum time on a path before another voluntary switch is allowed.
    pub min_dwell: Duration,

    /// Silence from a peer for this long marks the path dead and triggers
    /// immediate failover, bypassing hysteresis entirely (§85).
    pub path_timeout: Duration,

    /// Heartbeat interval on an idle path — VAD means a silent participant
    /// sends no media, so without this a healthy path looks identical to a
    /// dead one.
    pub heartbeat_interval: Duration,

    /// Keep a scored, ready standby path alongside the active one (§23).
    pub maintain_standby: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            weights: PathWeights::default(),
            lan_preference_bonus: 3.0,
            switch_hysteresis: 15.0,
            min_dwell: Duration::from_secs(10),
            path_timeout: Duration::from_secs(3),
            heartbeat_interval: Duration::from_millis(500),
            maintain_standby: true,
        }
    }
}

/// Relative importance of each path metric. Scores are 0–100 before weighting.
#[derive(Clone, Copy, Debug)]
pub struct PathWeights {
    /// Round-trip latency.
    pub latency: f32,
    /// Packet loss.
    pub loss: f32,
    /// Arrival variance.
    pub jitter: f32,
    /// How long the path has held up without incident.
    pub stability: f32,
    /// Forwarding hops (direct beats relayed).
    pub hops: f32,
    /// Battery/radio cost of the path.
    pub power: f32,
}

impl Default for PathWeights {
    fn default() -> Self {
        // Loss is weighted above latency deliberately. At the distances Anvil
        // operates over, every local path is fast; what actually destroys a
        // conversation is packets going missing.
        Self { latency: 0.25, loss: 0.30, jitter: 0.20, stability: 0.15, hops: 0.05, power: 0.05 }
    }
}

impl PathWeights {
    /// Sum of all weights. Scoring divides by this so weights need not be
    /// normalised by hand.
    #[must_use]
    pub fn total(&self) -> f32 {
        self.latency + self.loss + self.jitter + self.stability + self.hops + self.power
    }
}

/// Relay election configuration (§37–§40).
#[derive(Clone, Copy, Debug)]
pub struct RelayConfig {
    /// A challenger must beat the sitting relay by this margin to unseat it.
    ///
    /// Relay changes are expensive — every participant re-points its media — so
    /// the bar is higher than for a transport switch.
    pub election_hysteresis: f32,

    /// Minimum time a relay holds the role before a voluntary election.
    pub min_term: Duration,

    /// Missed relay heartbeats before the relay is declared dead.
    pub missed_heartbeats: u32,

    /// Below this battery percentage a device withdraws from candidacy.
    /// Relaying for a room is not something to do to someone at 8%.
    pub battery_floor_pct: u8,

    /// Refuse the relay role while on battery below `battery_floor_pct` even if
    /// no other candidate exists. False means "relay anyway rather than lose
    /// the room", which is usually the right call.
    pub hard_battery_floor: bool,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            election_hysteresis: 20.0,
            min_term: Duration::from_secs(30),
            missed_heartbeats: 3,
            battery_floor_pct: 15,
            hard_battery_floor: false,
        }
    }
}
