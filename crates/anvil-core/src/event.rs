//! Events the core emits to its host (§89).
//!
//! The host — Flutter, a test harness, a CLI — never reaches into protocol
//! state. It sends commands and receives events. That keeps the UI from
//! depending on internals it should not know about, and it means the same core
//! drives the app and the integration tests without a shim.
//!
//! Events describe *what happened*, never *what to display*. `TransportChanged`
//! is an event; "show the reconnecting spinner" is a UI decision made from it.

use crate::chat::{DeliveryState, Message, MessageId};
use crate::discovery::DiscoveredPeer;
use crate::identity::{Fingerprint, LocalProfile};
use crate::peer::{CallEnded, CallState};
use crate::room::{JoinCode, Participant, RoomSnapshot};
use crate::transport::PathKind;
use crate::{PeerId, RoomId};

/// Coarse node state (§88), mirrored in the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppState {
    /// Core constructed, adapters not yet ready.
    Initializing,
    /// Blocked on a permission the user must grant.
    PermissionsRequired,
    /// Idle, not scanning.
    Idle,
    /// Actively discovering peers.
    Discovering,
    /// Creating a room.
    CreatingRoom,
    /// Joining a room.
    JoiningRoom,
    /// In a room with a working path.
    Connected,
    /// In a room, path degraded or re-establishing. The room still exists —
    /// this is the state that proves session lifetime is decoupled from
    /// transport lifetime.
    Reconnecting,
    /// In a room, relay election in progress.
    RelayElection,
    /// Tearing down.
    Leaving,
    /// Unrecoverable for the current operation.
    Error,
}

/// Subjective quality of the current connection, for UI display.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionQuality {
    /// Clean.
    Good,
    /// Audible degradation likely.
    Fair,
    /// Conversation is suffering.
    Poor,
    /// No usable path right now.
    Lost,
}

/// Something the core wants the host to know about.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Event {
    /// Node state changed (§88).
    StateChanged {
        /// New state.
        state: AppState,
    },

    /// A nearby peer appeared, or an already-known peer gained a path.
    ///
    /// Fires once per *peer*, not once per path — §65 de-duplication happens in
    /// the core, so the UI never shows the same person twice.
    PeerDiscovered {
        /// The peer.
        peer: DiscoveredPeer,
    },

    /// A peer is no longer reachable on any path.
    PeerLost {
        /// The peer.
        peer_id: PeerId,
    },

    /// The local identity exists and is loaded.
    ///
    /// Emitted on first run after the user supplies a name, and on every
    /// subsequent launch after the stored identity is loaded. The UI leaves the
    /// first-run screen on this event and on nothing else — its absence is the
    /// only gate, and there is no login behind it.
    ProfileReady {
        /// This device's identity.
        profile: LocalProfile,
    },

    /// A peer completed an authenticated handshake and presented a **different
    /// key** for a name this device already trusted.
    ///
    /// Either they reinstalled Anvil — which genuinely regenerates identity —
    /// or somebody is impersonating them. Anvil cannot tell which and must not
    /// guess; this event exists so the UI can ask a human.
    IdentityChanged {
        /// Who is claiming the name.
        peer_id: PeerId,
        /// The name in question.
        display_name: String,
        /// The fingerprint previously trusted.
        previous_fingerprint: Fingerprint,
        /// The fingerprint now presented.
        new_fingerprint: Fingerprint,
    },

    /// A known peer is now going by a different name.
    PeerRenamed {
        /// Who.
        peer_id: PeerId,
        /// What we knew them as.
        previous_name: String,
        /// What they now call themselves.
        display_name: String,
    },

    /// Someone is calling.
    IncomingCall {
        /// Who.
        peer_id: PeerId,
        /// Their name, for the ringing screen.
        display_name: String,
    },

    /// The direct-call state machine moved.
    CallStateChanged {
        /// New state.
        state: CallState,
        /// Other party's name, when there is one.
        display_name: Option<String>,
    },

    /// A call finished, with the reason so the UI can distinguish "they hung
    /// up" from "we lost them".
    CallFinished {
        /// Who it was with.
        peer_id: Option<PeerId>,
        /// Why it ended.
        reason: CallEnded,
    },

    /// A message arrived.
    MessageReceived {
        /// The message.
        message: Message,
    },

    /// An outbound message's delivery state changed.
    MessageDelivery {
        /// Which message.
        id: MessageId,
        /// Where it got to.
        delivery: DeliveryState,
    },

    /// Local room created.
    RoomCreated {
        /// Its id.
        room_id: RoomId,
        /// The code to read out to whoever should join.
        join_code: JoinCode,
    },

    /// Local node joined a room.
    RoomJoined {
        /// Full room state at join.
        room: RoomSnapshot,
    },

    /// Local node left, or was removed.
    RoomLeft {
        /// Which room.
        room_id: RoomId,
        /// Why.
        reason: String,
    },

    /// Someone else joined.
    ParticipantJoined {
        /// The new participant.
        participant: Participant,
    },

    /// Someone else left.
    ParticipantLeft {
        /// Who.
        peer_id: PeerId,
    },

    /// A nearby authenticated peer asked to join a host-approved room (§68).
    ///
    /// The host UI calls `Command::RespondToJoin { peer_id, accept }` to admit
    /// or refuse. No membership state changes until that command arrives.
    JoinRequested {
        /// Who is asking.
        peer_id: PeerId,
        /// Display name they presented over their authenticated session.
        display_name: String,
    },

    /// A pending join request was refused by the host. The joiner's UI uses
    /// this to show a "room host declined" state rather than waiting forever.
    JoinDenied {
        /// Who was refused.
        peer_id: PeerId,
    },

    /// A participant started or stopped transmitting speech (VAD-driven).
    /// Drives the "who is talking" indicator.
    SpeakingChanged {
        /// Who.
        peer_id: PeerId,
        /// Whether they are speaking now.
        speaking: bool,
    },

    /// Media for a peer moved to a different path (§22).
    ///
    /// Note what this event does *not* contain: a room id change, a new peer
    /// id, or anything suggesting the session restarted. It did not.
    TransportChanged {
        /// Which peer.
        peer_id: PeerId,
        /// Path now carrying media.
        active: PathKind,
        /// Path held in reserve, if any.
        standby: Option<PathKind>,
    },

    /// The room's relay changed (§41).
    RelayChanged {
        /// New relay. `None` while an election is running.
        relay: Option<PeerId>,
        /// Human-readable cause, for diagnostics and the UI.
        reason: String,
    },

    /// Connection quality dropped.
    ConnectionDegraded {
        /// Affected peer, or `None` for the room as a whole.
        peer_id: Option<PeerId>,
        /// New quality.
        quality: ConnectionQuality,
    },

    /// Connection quality recovered.
    ConnectionRecovered {
        /// Affected peer, or `None` for the room as a whole.
        peer_id: Option<PeerId>,
    },

    /// Local mic mute state changed.
    MuteChanged {
        /// Whether the mic is muted.
        muted: bool,
    },

    /// Key epoch advanced after a membership change (§50).
    EpochChanged {
        /// The new epoch.
        epoch: crate::Epoch,
    },

    /// A permission must be granted before the named capability can be used.
    PermissionRequired {
        /// Capability name, e.g. `"microphone"`, `"nearby_wifi_devices"`.
        capability: &'static str,
    },

    /// Periodic diagnostics snapshot (§92). Only emitted when
    /// [`crate::AnvilConfig::diagnostics`] is set.
    Diagnostics {
        /// The snapshot.
        snapshot: crate::diagnostics::DiagnosticsSnapshot,
    },

    /// Something failed. Carries the layer via [`crate::Error`]'s variant so
    /// the host can distinguish "grant a permission" from "the network died".
    Error {
        /// Layer, e.g. `"transport"`.
        layer: &'static str,
        /// Message.
        message: String,
    },
}

/// Where events go.
///
/// A trait so the FFI layer, tests and any embedding host can each consume
/// events their own way. Implementations must not block: the engine emits from
/// its own loop, and a slow sink is indistinguishable from a stalled call.
pub trait EventSink: Send + Sync + core::fmt::Debug {
    /// Deliver one event. Drop rather than block if the consumer is behind.
    fn emit(&self, event: Event);
}

/// An event sink that throws everything away. Useful in tests that only care
/// about state, and as a safe default before a host attaches.
#[derive(Debug, Default, Clone, Copy)]
pub struct NullSink;

impl EventSink for NullSink {
    fn emit(&self, _event: Event) {}
}

/// An event sink that records everything, for tests.
#[derive(Debug, Default)]
pub struct RecordingSink {
    events: std::sync::Mutex<Vec<Event>>,
}

impl RecordingSink {
    /// Empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything recorded so far.
    #[must_use]
    pub fn events(&self) -> Vec<Event> {
        self.events.lock().expect("recording sink poisoned").clone()
    }

    /// Number of events recorded.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.lock().expect("recording sink poisoned").len()
    }

    /// Whether nothing has been recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: Event) {
        if let Ok(mut events) = self.events.lock() {
            events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_sink_collects_events() {
        let sink = RecordingSink::new();
        assert!(sink.is_empty());

        sink.emit(Event::StateChanged { state: AppState::Discovering });
        sink.emit(Event::MuteChanged { muted: true });

        assert_eq!(sink.len(), 2);
        assert!(matches!(sink.events()[0], Event::StateChanged { state: AppState::Discovering }));
    }
}
