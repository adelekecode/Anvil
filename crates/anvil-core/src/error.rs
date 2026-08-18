//! Error boundaries.
//!
//! Spec §91 asks for these to stay separate, and it is right to: when voice
//! drops in a room, the single most valuable piece of information is *which
//! layer* gave up. A flat error type turns every failure into "something went
//! wrong on the network", which is exactly the debugging experience this
//! project cannot afford.
//!
//! The rule for adding a variant: name the *condition*, not the call that
//! failed. `RelayUnreachable` is useful; `SendFailed` is not.

use crate::{PathId, PeerId, RoomId};

/// Convenience alias.
pub type Result<T, E = Error> = core::result::Result<T, E>;

/// Top-level error. Always carries the layer that produced it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The OS or a native adapter refused or failed.
    #[error("platform: {0}")]
    Platform(#[from] PlatformError),

    /// Path establishment, path loss, or send/receive failure.
    #[error("transport: {0}")]
    Transport(#[from] TransportError),

    /// A peer sent something malformed, unsupported, or out of sequence.
    #[error("protocol: {0}")]
    Protocol(#[from] ProtocolError),

    /// Key agreement, authentication, or AEAD failure.
    #[error("crypto: {0}")]
    Crypto(#[from] CryptoError),

    /// Capture, encode, decode or playback failure.
    #[error("audio: {0}")]
    Audio(#[from] AudioError),

    /// Room membership or lifecycle failure.
    #[error("room: {0}")]
    Room(#[from] RoomError),

    /// Reached a Phase 0 stub. Carries the module and what still has to be
    /// built there, so this is actionable rather than mysterious.
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
}

/// Failures originating in a Kotlin/Swift adapter or the OS beneath it.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    /// The user has not granted a permission the operation needs.
    #[error("permission not granted: {0}")]
    PermissionDenied(&'static str),

    /// The hardware or OS version does not offer this capability at all.
    ///
    /// Wi-Fi Aware in particular is not universally available; this is an
    /// expected condition on plenty of shipping devices, not a bug.
    #[error("capability unavailable on this device: {0}")]
    Unsupported(&'static str),

    /// No adapter has been registered for a capability the core tried to use.
    #[error("no platform adapter registered")]
    NoAdapter,

    /// The adapter reported a failure of its own.
    #[error("adapter error: {0}")]
    Adapter(String),
}

/// Failures in path establishment or use.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// No usable path to this peer, on any transport.
    #[error("no path to peer {0}")]
    NoPath(PeerId),

    /// A specific path failed and has been torn down.
    #[error("path {0:?} failed: {1}")]
    PathFailed(PathId, String),

    /// The path exists but the peer stopped answering.
    #[error("peer {0} timed out")]
    PeerTimeout(PeerId),

    /// Datagram exceeded what the path can carry unfragmented.
    #[error("datagram too large: {size} bytes, path limit {limit}")]
    DatagramTooLarge {
        /// Size we tried to send.
        size: usize,
        /// What the path allows.
        limit: usize,
    },

    /// Underlying QUIC/socket failure.
    #[error("io: {0}")]
    Io(String),
}

/// Failures parsing or interpreting peer traffic.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// Peer speaks a wire version this build cannot handle.
    #[error("unsupported protocol version {theirs} (this build speaks {ours})")]
    VersionMismatch {
        /// What they advertised.
        theirs: u8,
        /// What we speak.
        ours: u8,
    },

    /// Buffer ended before the header did.
    #[error("truncated packet: {got} bytes, need at least {need}")]
    Truncated {
        /// Bytes available.
        got: usize,
        /// Bytes required.
        need: usize,
    },

    /// Header decoded but is nonsensical.
    #[error("malformed packet: {0}")]
    Malformed(&'static str),

    /// Packet type byte is not one we know.
    #[error("unknown packet type {0:#04x}")]
    UnknownPacketType(u8),

    /// Control message arrived in a state that cannot accept it.
    #[error("unexpected message {message} in state {state}")]
    UnexpectedMessage {
        /// The message received.
        message: &'static str,
        /// The state we were in.
        state: &'static str,
    },
}

/// Failures in identity, key agreement or media encryption.
///
/// Deliberately vague in its `Display` output where an attacker might be
/// listening: distinguishing "bad tag" from "unknown epoch" in a log that ships
/// somewhere is a small oracle. Detail belongs in `Debug`, not user-facing text.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    /// AEAD authentication failed — packet discarded, never decoded (§80).
    #[error("authentication failed")]
    AuthenticationFailed,

    /// Sequence/nonce indicates a replay of already-accepted traffic (§81).
    #[error("replay rejected")]
    ReplayRejected,

    /// We hold no key material for the epoch this packet claims.
    #[error("no key material for {0}")]
    UnknownEpoch(crate::Epoch),

    /// Peer's identity signature did not verify.
    #[error("peer identity verification failed")]
    IdentityRejected,

    /// The handshake did not complete.
    #[error("handshake failed: {0}")]
    HandshakeFailed(&'static str),

    /// Platform secure storage refused to store or return the identity key.
    #[error("key storage: {0}")]
    KeyStorage(String),
}

/// Failures in the audio pipeline.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// Capture device unavailable or lost (call interruption, route change).
    #[error("capture unavailable: {0}")]
    CaptureUnavailable(String),

    /// Playback device unavailable or lost.
    #[error("playback unavailable: {0}")]
    PlaybackUnavailable(String),

    /// Opus encode failed.
    #[error("encode failed: {0}")]
    Encode(String),

    /// Opus decode failed.
    #[error("decode failed: {0}")]
    Decode(String),

    /// Frame did not match the negotiated sample rate/channels/duration.
    #[error("unexpected frame format: {0}")]
    FrameFormat(&'static str),
}

/// Failures in room lifecycle or membership.
#[derive(Debug, thiserror::Error)]
pub enum RoomError {
    /// Operation needs a room and there is none.
    #[error("not in a room")]
    NotInRoom,

    /// Tried to join or create while already in a room.
    #[error("already in room {0}")]
    AlreadyInRoom(RoomId),

    /// Referenced a room we know nothing about.
    #[error("unknown room {0}")]
    UnknownRoom(RoomId),

    /// Referenced a peer who is not a member.
    #[error("peer {0} is not a member")]
    NotAMember(PeerId),

    /// The host declined the join request.
    #[error("join rejected: {0}")]
    JoinRejected(String),

    /// Room emptied out or all peers became unreachable.
    #[error("room collapsed: no reachable participants")]
    Collapsed,
}
