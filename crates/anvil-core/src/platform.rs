//! The Rust ↔ native boundary (§90).
//!
//! This is the most important interface in the project, because it is the one
//! that keeps `#[cfg(target_os = ...)]` out of the protocol. The contract is:
//!
//! > **Adapters expose capabilities. The core makes decisions.**
//!
//! An adapter is told "publish this service", "open a path to this endpoint",
//! "give me microphone frames". It is never told *why*, and it never decides
//! which transport to use, when to fail over, who relays, or whether a peer is
//! trustworthy. Those are protocol questions and they are answered in Rust,
//! once, for both platforms.
//!
//! Concretely this means an adapter must not:
//!
//! * retry a failed connection on its own — the core scores paths and decides;
//! * pick between LAN and Wi-Fi Aware — it reports both and lets the core choose;
//! * de-duplicate discovered peers — correlation is cryptographic (§65) and the
//!   adapter has no identity information to do it with;
//! * buffer or reorder media — the jitter buffer lives in the core so that its
//!   behaviour is identical on both platforms and testable off-device.
//!
//! Everything flows the other way as [`PlatformEvent`]. Adapters push; they are
//! never polled.

use crate::audio::PcmFrame;
use crate::discovery::PeerAdvertisement;
use crate::transport::{Endpoint, PathKind};
use crate::{PathId, PlatformError, Result};

/// Something that happened on the device, normalised across Android and iOS.
///
/// The whole point of this enum is that a Kotlin `WifiAwareSession` callback
/// and a Swift `NWBrowser` result arrive at the core looking identical.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PlatformEvent {
    /// A device advertising the Anvil service was seen.
    ///
    /// This is a *raw sighting*, not a peer. The same physical phone seen over
    /// LAN and over Wi-Fi Aware produces two of these; the core folds them into
    /// one identity after the handshake.
    PeerAdvertised {
        /// What was seen, and where.
        advertisement: PeerAdvertisement,
    },

    /// An advertisement expired or the service went away.
    PeerAdvertisementLost {
        /// Which transport saw it disappear.
        kind: PathKind,
        /// Adapter-local handle from the original advertisement.
        handle: String,
    },

    /// A path finished establishing and can carry traffic.
    PathEstablished {
        /// Core-assigned id, echoed back from the connect request.
        path: PathId,
        /// Largest datagram this path carries unfragmented. The core needs
        /// this to size Opus frames; Wi-Fi Aware and LAN do not agree on it.
        max_datagram_size: usize,
    },

    /// A path failed or was closed.
    ///
    /// Hard loss — this bypasses scoring hysteresis and triggers immediate
    /// failover (§85).
    PathLost {
        /// Which path.
        path: PathId,
        /// Why, for diagnostics.
        reason: String,
    },

    /// Bytes arrived on a path.
    DatagramReceived {
        /// Which path.
        path: PathId,
        /// Raw bytes. Still encrypted; the core authenticates before decoding.
        data: Vec<u8>,
    },

    /// Reliable, ordered control bytes arrived on a QUIC stream.
    ReliableReceived {
        /// Path carrying the authenticated transport session.
        path: PathId,
        /// One complete control message.
        data: Vec<u8>,
    },

    /// The device's network situation changed — Wi-Fi joined or left, interface
    /// up or down, Aware availability toggled.
    ///
    /// A hint to re-evaluate paths, not an instruction to switch.
    NetworkChanged {
        /// Which transport family changed.
        kind: PathKind,
        /// Whether it is usable now.
        available: bool,
    },

    /// A microphone frame is ready.
    AudioCaptured {
        /// Raw PCM at the configured rate and channel count.
        frame: PcmFrame,
    },

    /// Audio route changed — headset plugged in, Bluetooth connected, speaker
    /// switched. Note this is about the *user's* audio hardware and has nothing
    /// to do with Anvil's peer transport (§87).
    AudioRouteChanged {
        /// Human-readable route description for diagnostics.
        route: String,
    },

    /// Audio was interrupted by the system — a phone call, another app taking
    /// the session. Capture and playback are stopped until this clears.
    AudioInterrupted {
        /// Whether the interruption has ended.
        resumed: bool,
    },

    /// A permission was granted or revoked.
    PermissionChanged {
        /// Capability name.
        capability: &'static str,
        /// Whether it is granted now.
        granted: bool,
    },

    /// Periodic device status, used by relay election (§37) and path power
    /// scoring.
    DeviceStatus {
        /// Battery charge, if known.
        battery_pct: Option<u8>,
        /// Whether the device is on external power. A plugged-in phone is a
        /// far better relay candidate than one on battery.
        charging: bool,
        /// Whether the OS reports thermal pressure. A throttled device makes a
        /// poor relay regardless of how good its network looks.
        thermally_throttled: bool,
    },

    /// App moved between foreground and background. Both platforms restrict
    /// what a backgrounded app may do with radios and audio, so the core needs
    /// to know.
    LifecycleChanged {
        /// Whether the app is in the foreground.
        foreground: bool,
    },
}

/// What a device can actually do.
///
/// Queried once at startup and re-checked on [`PlatformEvent::NetworkChanged`].
/// Wi-Fi Aware in particular is absent on a large fraction of shipping hardware,
/// and Anvil must degrade to LAN-only gracefully rather than treating that as
/// an error.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    /// LAN discovery and connectivity usable.
    pub lan: bool,
    /// Wi-Fi Aware supported by hardware *and* OS *and* currently enabled.
    pub wifi_aware: bool,
    /// Microphone permission granted.
    pub microphone: bool,
    /// Nearby-devices / local-network permission granted.
    pub nearby_devices: bool,
    /// Hardware-backed key storage available for the identity key (§82).
    pub secure_key_storage: bool,
}

/// Local network discovery and Wi-Fi Aware discovery.
pub trait DiscoveryAdapter: Send + Sync + core::fmt::Debug {
    /// Begin browsing for the Anvil service over local network discovery
    /// (NSD on Android, Bonjour/`NWBrowser` on iOS).
    fn start_lan_discovery(&self) -> Result<()>;

    /// Stop LAN browsing.
    fn stop_lan_discovery(&self) -> Result<()>;

    /// Begin publishing and subscribing over Wi-Fi Aware.
    fn start_aware_discovery(&self) -> Result<()>;

    /// Stop Wi-Fi Aware discovery.
    fn stop_aware_discovery(&self) -> Result<()>;

    /// Advertise this node on every discovery mechanism currently running.
    ///
    /// `payload` is an opaque blob built by the core — it carries the peer's
    /// public identity fingerprint and, if hosting, a room advertisement. The
    /// adapter copies it into the service TXT record or Aware service info
    /// without interpreting it.
    fn advertise(&self, payload: &[u8]) -> Result<()>;

    /// Stop advertising.
    fn stop_advertising(&self) -> Result<()>;
}

/// Path establishment and datagram/stream I/O.
pub trait TransportAdapter: Send + Sync + core::fmt::Debug {
    /// Begin establishing a path to `endpoint`.
    ///
    /// Returns immediately; success or failure arrives as
    /// [`PlatformEvent::PathEstablished`] or [`PlatformEvent::PathLost`]
    /// carrying `path`. The core assigns the [`PathId`] so it can track a
    /// pending path before the adapter has anything to name it with.
    fn connect(&self, path: PathId, endpoint: &Endpoint) -> Result<()>;

    /// Tear down a path.
    fn close(&self, path: PathId) -> Result<()>;

    /// Send an unreliable datagram — media, probes, heartbeats.
    ///
    /// Must not queue on congestion. A late voice frame is worse than a lost
    /// one, and buffering here would defeat the jitter buffer at the far end.
    fn send_datagram(&self, path: PathId, data: &[u8]) -> Result<()>;

    /// Send reliable ordered control traffic — join, membership, key
    /// distribution, election (§25).
    fn send_reliable(&self, path: PathId, data: &[u8]) -> Result<()>;

    /// Bind a local listener so peers can connect inbound, returning the
    /// endpoint to advertise.
    fn listen(&self, kind: PathKind) -> Result<Endpoint>;
}

/// Microphone capture and speaker playback.
pub trait AudioAdapter: Send + Sync + core::fmt::Debug {
    /// Start capture. Frames arrive as [`PlatformEvent::AudioCaptured`].
    fn start_capture(&self, config: &crate::AudioConfig) -> Result<()>;

    /// Stop capture.
    fn stop_capture(&self) -> Result<()>;

    /// Start the playback path.
    fn start_playback(&self, config: &crate::AudioConfig) -> Result<()>;

    /// Stop playback.
    fn stop_playback(&self) -> Result<()>;

    /// Hand one mixed PCM frame to the output device.
    ///
    /// Called from the core's audio timing path. Must not block: it runs on the
    /// cadence of the frame duration, and overrunning it is an underrun the
    /// user hears.
    fn play(&self, frame: &PcmFrame) -> Result<()>;
}

/// Long-lived identity key storage, backed by Keychain / Android Keystore (§82).
pub trait KeyStoreAdapter: Send + Sync + core::fmt::Debug {
    /// Load the stored identity key, or `None` on first run.
    fn load_identity(&self) -> Result<Option<Vec<u8>>>;

    /// Persist the identity key in hardware-backed storage where available.
    fn store_identity(&self, bytes: &[u8]) -> Result<()>;

    /// Destroy the stored identity. Irreversible — the device becomes a new
    /// [`crate::PeerId`] to everyone who has met it.
    fn clear_identity(&self) -> Result<()>;
}

/// Everything the core needs from a device.
///
/// Implemented once per platform, in `apps/mobile/android/.../AnvilPlatform.kt`
/// and `apps/mobile/ios/Runner/AnvilPlatform.swift`, and once more in-memory
/// for tests. That third implementation is the point: the entire protocol,
/// including relay election and path failover, is exercisable without a phone.
pub trait PlatformAdapter:
    DiscoveryAdapter + TransportAdapter + AudioAdapter + KeyStoreAdapter
{
    /// What this device supports right now.
    fn capabilities(&self) -> Capabilities;

    /// Request a permission from the user. Result arrives as
    /// [`PlatformEvent::PermissionChanged`].
    fn request_permission(&self, capability: &'static str) -> Result<()>;
}

/// A [`PlatformAdapter`] that supports nothing and fails every call.
///
/// The default before a host installs a real one, so that constructing an
/// [`crate::Engine`] never requires a device. Every method returns
/// [`PlatformError::NoAdapter`] rather than panicking — a headless build should
/// report "no adapter" cleanly, not abort.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullPlatform;

impl NullPlatform {
    fn no_adapter<T>() -> Result<T> {
        Err(PlatformError::NoAdapter.into())
    }
}

impl DiscoveryAdapter for NullPlatform {
    fn start_lan_discovery(&self) -> Result<()> {
        Self::no_adapter()
    }
    fn stop_lan_discovery(&self) -> Result<()> {
        Ok(())
    }
    fn start_aware_discovery(&self) -> Result<()> {
        Self::no_adapter()
    }
    fn stop_aware_discovery(&self) -> Result<()> {
        Ok(())
    }
    fn advertise(&self, _payload: &[u8]) -> Result<()> {
        Self::no_adapter()
    }
    fn stop_advertising(&self) -> Result<()> {
        Ok(())
    }
}

impl TransportAdapter for NullPlatform {
    fn connect(&self, _path: PathId, _endpoint: &Endpoint) -> Result<()> {
        Self::no_adapter()
    }
    fn close(&self, _path: PathId) -> Result<()> {
        Ok(())
    }
    fn send_datagram(&self, _path: PathId, _data: &[u8]) -> Result<()> {
        Self::no_adapter()
    }
    fn send_reliable(&self, _path: PathId, _data: &[u8]) -> Result<()> {
        Self::no_adapter()
    }
    fn listen(&self, _kind: PathKind) -> Result<Endpoint> {
        Self::no_adapter()
    }
}

impl AudioAdapter for NullPlatform {
    fn start_capture(&self, _config: &crate::AudioConfig) -> Result<()> {
        Self::no_adapter()
    }
    fn stop_capture(&self) -> Result<()> {
        Ok(())
    }
    fn start_playback(&self, _config: &crate::AudioConfig) -> Result<()> {
        Self::no_adapter()
    }
    fn stop_playback(&self) -> Result<()> {
        Ok(())
    }
    fn play(&self, _frame: &PcmFrame) -> Result<()> {
        Self::no_adapter()
    }
}

impl KeyStoreAdapter for NullPlatform {
    fn load_identity(&self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    fn store_identity(&self, _bytes: &[u8]) -> Result<()> {
        Self::no_adapter()
    }
    fn clear_identity(&self) -> Result<()> {
        Ok(())
    }
}

impl PlatformAdapter for NullPlatform {
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }
    fn request_permission(&self, _capability: &'static str) -> Result<()> {
        Self::no_adapter()
    }
}
