//! # anvil-core
//!
//! The Anvil protocol. Everything that decides *what Anvil does* lives here:
//! identity, rooms, membership, cryptography, the media pipeline, packet
//! format, transport selection, relay election and failure recovery.
//!
//! Two hard rules shape this crate:
//!
//! 1. **The core owns policy; the platform owns mechanism.** This crate never
//!    calls an OS API. It asks a [`PlatformAdapter`] to publish a service, open
//!    a socket, or hand it a microphone frame, and it decides what to do with
//!    the result. There is no `#[cfg(target_os = "android")]` in the protocol.
//! 2. **The logical session outlives the physical path.** [`RoomId`],
//!    [`PeerId`], [`StreamId`], sequence state and key epochs are all defined
//!    independently of any socket, IP address or radio. A path can die and be
//!    replaced without the room noticing.
//!
//! ## Shape of the thing
//!
//! ```text
//!   Flutter / any host
//!         │  Command                    Event  ▲
//!         ▼                                    │
//!   ┌──────────────────────────────────────────────┐
//!   │  Engine  (single owner of all mutable state) │
//!   │    RoomState · PeerTable · TransportManager  │
//!   │    RelayElection · GroupKeyManager · Audio   │
//!   └──────────────────────────────────────────────┘
//!         │  PlatformCommand           PlatformEvent  ▲
//!         ▼                                           │
//!   Kotlin / Swift adapters → Wi-Fi LAN · Wi-Fi Aware · mic · speaker
//! ```
//!
//! The [`Engine`] is the only thing that mutates state, and it does so from one
//! thread driven by a single inbox. Every other type here is either a value, a
//! pure function over values, or a trait the platform implements. That is
//! deliberate: concurrency bugs in a real-time voice stack are miserable to
//! find, so there is exactly one place where ordering matters.
//!
//! ## Phase 0 status
//!
//! This is the interface skeleton described by the architecture spec. Types,
//! traits and module boundaries are real and are meant to be stable. Bodies
//! that need Phase 1+ work return [`Error::NotImplemented`] or an explicit
//! placeholder rather than pretending to work. Search for `PHASE1:` / `PHASE2:`
//! markers to find them.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod audio;
pub mod chat;
pub mod crypto;
pub mod diagnostics;
pub mod discovery;
pub mod identity;
pub mod peer;
pub mod platform;
pub mod protocol;
pub mod relay;
pub mod room;
pub mod routing;
pub mod transport;

mod config;
mod engine;
mod error;
mod event;
mod ids;
mod time;

pub use config::{AnvilConfig, AudioConfig, RelayConfig, TransportConfig};
pub use engine::{Command, Engine, EngineHandle};
pub use error::{AudioError, CryptoError, Error, PlatformError, ProtocolError, Result, RoomError,
                TransportError};
pub use event::{AppState, ConnectionQuality, Event, EventSink, NullSink, RecordingSink};
pub use ids::{Epoch, PathId, PeerId, RoomId, SeqNum, StreamId};
pub use platform::{PlatformAdapter, PlatformEvent};
pub use time::{Clock, MediaTimestamp, Monotonic, SystemClock, TestClock};

/// Wire protocol version this build speaks.
///
/// Bumped on any incompatible change to the packet format. Peers advertising a
/// version this build cannot handle are refused at handshake rather than
/// half-supported.
pub const PROTOCOL_VERSION: u8 = 1;

/// Service name Anvil advertises over LAN discovery and Wi-Fi Aware.
///
/// Kept identical across both transports on purpose — it is one of the signals
/// used to correlate a peer seen twice into one [`PeerId`].
pub const SERVICE_NAME: &str = "_anvil._udp";
