//! Transport abstraction, path scoring and failover.
//!
//! The central claim of this module is §21: **a room session is not a
//! connection**. `RoomId`, `PeerId`, `StreamId`, sequence state and key epochs
//! live above everything here. A path can die and be replaced and the
//! conversation continues, because nothing above this layer ever learned the
//! path existed.
//!
//! ```text
//!   RoomSession        ← survives everything below
//!       │
//!   PeerSession        ← one per participant, survives path changes
//!       │
//!   TransportManager   ← owns candidate paths, scores them, switches
//!       │
//!   Path (LAN)  Path (Aware)  ← disposable
//! ```

mod manager;
mod metrics;
mod path;
mod scoring;

pub mod aware;
pub mod lan;
pub mod quic;

pub use manager::{PeerConnection, TransportManager};
pub use metrics::{PathMetrics, PathSample};
pub use path::{Path, PathState};
pub use scoring::{score_path, should_switch, SwitchDecision};

use core::fmt;

/// Which physical transport a path runs over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PathKind {
    /// Existing Wi-Fi infrastructure network. Internet not required or assumed
    /// — the router only has to provide local IP connectivity (§20).
    Lan,
    /// Wi-Fi Aware peer-to-peer, for when there is no router at all.
    WifiAware,
}

impl PathKind {
    /// Rough relative power cost, 0.0 (cheap) to 1.0 (expensive), feeding the
    /// power term in [`score_path`].
    ///
    /// Wi-Fi Aware costs more: the radio does its own discovery and clustering
    /// duty-cycling rather than riding an association the phone is maintaining
    /// anyway. Treat this as a placeholder until §93 measurements exist.
    #[must_use]
    pub const fn power_cost(self) -> f32 {
        match self {
            Self::Lan => 0.2,
            Self::WifiAware => 0.6,
        }
    }
}

impl fmt::Display for PathKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lan => "lan",
            Self::WifiAware => "wifi-aware",
        })
    }
}

/// Where to reach a peer on a given transport.
///
/// Opaque to the protocol on purpose. The core moves these around and hands
/// them back to the adapter; it never parses one, because the day it does is
/// the day LAN details leak into code that also has to work over Wi-Fi Aware.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Endpoint {
    /// Transport this endpoint belongs to.
    pub kind: PathKind,
    /// Adapter-defined address. A socket address string for LAN; an Aware
    /// peer/session handle for Wi-Fi Aware.
    pub address: String,
}

impl Endpoint {
    /// Build an endpoint.
    #[must_use]
    pub fn new(kind: PathKind, address: impl Into<String>) -> Self {
        Self { kind, address: address.into() }
    }
}

impl fmt::Debug for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Addresses are mildly identifying; keep them out of casual logs.
        write!(f, "Endpoint({}, {} chars)", self.kind, self.address.len())
    }
}
