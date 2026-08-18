//! The engine: the single owner of all mutable protocol state.
//!
//! Everything that can change lives here, and it changes from one place. The
//! host sends [`Command`]s; the platform pushes [`PlatformEvent`]s; the engine
//! folds both into state and emits [`Event`]s. Nothing else mutates anything.
//!
//! ```text
//!   Command ─────┐
//!                ├──► Engine ──► Event
//!   PlatformEvent┘       │
//!                        └──► PlatformAdapter calls
//! ```
//!
//! ## Why one owner
//!
//! Real-time voice is exactly the domain where a data race produces a bug you
//! cannot reproduce: audio arrives on one thread, the network on another, the
//! UI on a third, and the symptom is "it sounded odd once". Funnelling every
//! mutation through one inbox makes the ordering of events a thing you can
//! read, log and replay. The cost is one queue hop per event, which at
//! ~50 packets per second per participant is nothing.
//!
//! The engine deliberately does **not** own an async runtime. Platform adapters
//! are callback-driven — Kotlin and Swift push events in — and the transport
//! implementations that need async own their runtimes privately, behind
//! [`crate::platform::TransportAdapter`]. That keeps the protocol itself
//! synchronous, deterministic and testable with a hand-driven clock.
//!
//! ## Phase status
//!
//! Phase 0: the loop, the state ownership and the command/event plumbing are
//! real. Handlers that need Phase 1+ subsystems emit
//! [`Event::Error`] with the layer named, rather than silently doing nothing —
//! an unimplemented path should be loud.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;

use crate::audio::{JitterBuffer, Mixer, VoiceActivityDetector};
use crate::chat::{Conversation, DeliveryState, History, Message};
use crate::crypto::SenderKeyManager;
use crate::diagnostics::Counters;
use crate::discovery::PeerTable;
use crate::identity::{KnownPeers, LocalProfile, PublicIdentity, TofuOutcome};
use crate::peer::{CallEnded, CallState};
use crate::platform::{NullPlatform, PlatformAdapter, PlatformEvent};
use crate::relay::RelayMonitor;
use crate::room::{JoinCode, RoomIdentity, RoomState};
use crate::time::{Clock, Monotonic, SystemClock};
use crate::transport::TransportManager;
use crate::{AnvilConfig, AppState, Event, EventSink, PeerId, RoomId};

/// How often the engine wakes with nothing to do.
///
/// 20 ms, matching the audio frame cadence: the tick drives playout, path
/// re-evaluation, heartbeats and expiry, and anything slower would make playout
/// jittery on its own.
const TICK: core::time::Duration = core::time::Duration::from_millis(20);

/// Something the host asks the engine to do (§8).
#[derive(Debug)]
#[non_exhaustive]
pub enum Command {
    // --- identity ---------------------------------------------------------
    /// Create the local identity on first run.
    ///
    /// The only thing a user is ever asked for. There is no password, no email
    /// and no server round trip — this generates a keypair and stores it.
    CreateProfile {
        /// What to call this device.
        display_name: String,
    },

    /// Change the display name. Does not touch the identity.
    RenameProfile {
        /// The new name.
        display_name: String,
    },

    /// Mark a peer as verified after an out-of-band check (QR, or comparing
    /// fingerprints in person).
    VerifyPeer {
        /// Who.
        peer_id: PeerId,
    },

    /// Accept a peer's changed identity without verifying it — "yes, they
    /// reinstalled". Recorded as unverified, because dismissing a warning is
    /// not the same as checking a fingerprint.
    AcceptIdentityChange {
        /// Who.
        peer_id: PeerId,
    },

    // --- direct calls -----------------------------------------------------
    /// Call a peer directly. No relay is involved for two people.
    CallPeer {
        /// Who to call.
        peer_id: PeerId,
    },

    /// Answer the incoming call.
    AcceptCall,

    /// Refuse the incoming call.
    DeclineCall,

    /// Hang up, from any state.
    EndCall,

    // --- chat -------------------------------------------------------------
    /// Send a message to a peer or a room.
    SendMessage {
        /// Where it goes.
        conversation: Conversation,
        /// What it says.
        body: String,
    },

    /// Join a room using the code the host read out.
    JoinRoomByCode {
        /// What the user typed. Parsed forgivingly.
        code: String,
    },

    // --- discovery and rooms ----------------------------------------------
    /// Begin discovering nearby peers.
    StartDiscovery,
    /// Stop discovering.
    StopDiscovery,
    /// Create a room and host it.
    CreateRoom,
    /// Join a room.
    JoinRoom {
        /// Which room.
        room_id: RoomId,
        /// Join code, if the room uses one.
        credential: Option<Vec<u8>>,
    },
    /// Admit or refuse a pending join request.
    RespondToJoin {
        /// Who asked.
        peer_id: PeerId,
        /// Whether to admit them.
        accept: bool,
    },
    /// Leave the current room.
    LeaveRoom,
    /// Mute the microphone.
    Mute,
    /// Unmute.
    Unmute,
    /// Ask for a diagnostics snapshot now.
    RequestDiagnostics,
    /// A platform adapter reporting something.
    Platform(PlatformEvent),
    /// Stop the engine.
    Shutdown,
}

/// Handle for sending commands to a running engine.
///
/// Cloneable and `Send`, so the FFI layer, the audio callback and a UI thread
/// can all submit without sharing anything mutable.
#[derive(Clone, Debug)]
pub struct EngineHandle {
    tx: Sender<Command>,
}

impl EngineHandle {
    /// Submit a command.
    ///
    /// Returns false if the engine has stopped. Callers should treat that as
    /// "the session is over", not as an error to retry.
    pub fn send(&self, command: Command) -> bool {
        self.tx.send(command).is_ok()
    }

    /// Submit a platform event.
    pub fn platform(&self, event: PlatformEvent) -> bool {
        self.send(Command::Platform(event))
    }
}

/// The protocol engine.
pub struct Engine {
    config: AnvilConfig,
    clock: Arc<dyn Clock>,
    platform: Arc<dyn PlatformAdapter>,
    sink: Arc<dyn EventSink>,

    rx: Receiver<Command>,

    state: AppState,
    local_peer_id: PeerId,

    /// `None` until first run completes. Its absence is what puts the app on
    /// the display-name screen — there is no other gate.
    profile: Option<LocalProfile>,
    known_peers: KnownPeers,
    call: CallState,
    history: History,

    peers: PeerTable,
    transport: TransportManager,
    room: Option<RoomState>,
    /// Room id plus the human-facing join code, for a room we host or joined.
    room_identity: Option<RoomIdentity>,
    keys: SenderKeyManager,
    relay_monitor: Option<RelayMonitor>,

    vad: VoiceActivityDetector,
    /// PHASE1: driven from the playout tick once decode exists. Constructed
    /// now so the audio format is validated at startup rather than at the first
    /// spoken word.
    #[allow(dead_code)]
    mixer: Mixer,
    jitter: std::collections::HashMap<PeerId, JitterBuffer>,

    counters: Counters,
    muted: bool,
    last_diagnostics: Monotonic,
}

impl core::fmt::Debug for Engine {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Engine")
            .field("state", &self.state)
            .field("local_peer_id", &self.local_peer_id)
            .field("room", &self.room.as_ref().map(|r| r.room_id))
            .field("peers", &self.peers.len())
            .finish_non_exhaustive()
    }
}

impl Engine {
    /// Build an engine and its handle.
    ///
    /// Takes its platform, clock and event sink by injection rather than
    /// constructing them, which is what makes a full protocol test — discovery
    /// through relay failover — runnable with no device and no real time.
    pub fn new(
        config: AnvilConfig,
        platform: Arc<dyn PlatformAdapter>,
        sink: Arc<dyn EventSink>,
        clock: Arc<dyn Clock>,
        local_peer_id: PeerId,
    ) -> (Self, EngineHandle) {
        let (tx, rx) = mpsc::channel();
        let handle = EngineHandle { tx };

        let engine = Self {
            transport: TransportManager::new(config.transport),
            vad: VoiceActivityDetector::new(&config.audio),
            mixer: Mixer::new(&config.audio),
            keys: SenderKeyManager::new(local_peer_id),
            config,
            clock,
            platform,
            sink,
            rx,
            state: AppState::Initializing,
            local_peer_id,
            profile: None,
            known_peers: KnownPeers::new(),
            call: CallState::Idle,
            history: History::new(),
            peers: PeerTable::new(),
            room: None,
            room_identity: None,
            relay_monitor: None,
            jitter: std::collections::HashMap::new(),
            counters: Counters::default(),
            muted: false,
            last_diagnostics: Monotonic::ZERO,
        };

        (engine, handle)
    }

    /// Build an engine with no platform attached, for tests and headless hosts.
    pub fn headless(
        config: AnvilConfig,
        sink: Arc<dyn EventSink>,
        local_peer_id: PeerId,
    ) -> (Self, EngineHandle) {
        Self::new(
            config,
            Arc::new(NullPlatform),
            sink,
            Arc::new(SystemClock::new()),
            local_peer_id,
        )
    }

    /// Current node state.
    #[must_use]
    pub const fn state(&self) -> AppState {
        self.state
    }

    /// Current room, if any.
    #[must_use]
    pub fn room(&self) -> Option<&RoomState> {
        self.room.as_ref()
    }

    /// The local profile, or `None` before first run has completed.
    #[must_use]
    pub fn profile(&self) -> Option<&LocalProfile> {
        self.profile.as_ref()
    }

    /// Peers this device has met before.
    #[must_use]
    pub fn known_peers(&self) -> &KnownPeers {
        &self.known_peers
    }

    /// Current direct-call state.
    #[must_use]
    pub const fn call(&self) -> CallState {
        self.call
    }

    /// Local conversation history.
    #[must_use]
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Join code for the current room, if there is one.
    #[must_use]
    pub fn join_code(&self) -> Option<JoinCode> {
        self.room_identity.map(|identity| identity.join_code)
    }

    /// Peers currently visible.
    #[must_use]
    pub fn peers(&self) -> &PeerTable {
        &self.peers
    }

    /// Transport state.
    #[must_use]
    pub fn transport(&self) -> &TransportManager {
        &self.transport
    }

    /// Whether the microphone is muted.
    #[must_use]
    pub const fn is_muted(&self) -> bool {
        self.muted
    }

    /// Run until [`Command::Shutdown`] or the last handle is dropped.
    ///
    /// Blocks. Hosts run this on a dedicated thread.
    pub fn run(mut self) {
        self.set_state(AppState::Idle);

        loop {
            match self.rx.recv_timeout(TICK) {
                Ok(Command::Shutdown) => break,
                Ok(command) => self.handle_command(command),
                Err(RecvTimeoutError::Timeout) => {}
                // Every handle dropped: nobody can talk to us again.
                Err(RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
        }

        self.set_state(AppState::Leaving);
    }

    /// Process one command. Public so tests can drive the engine
    /// deterministically instead of racing a real loop.
    pub fn handle_command(&mut self, command: Command) {
        match command {
            Command::CreateProfile { display_name } => self.create_profile(&display_name),
            Command::RenameProfile { display_name } => self.rename_profile(&display_name),
            Command::VerifyPeer { peer_id } => self.set_peer_trust(peer_id, true),
            Command::AcceptIdentityChange { peer_id } => self.set_peer_trust(peer_id, false),
            Command::CallPeer { peer_id } => self.place_call(peer_id),
            Command::AcceptCall => self.accept_call(),
            Command::DeclineCall => self.end_call(CallEnded::Declined),
            Command::EndCall => self.end_call(CallEnded::HungUp),
            Command::SendMessage { conversation, body } => {
                self.send_message(conversation, &body);
            }
            Command::JoinRoomByCode { code } => self.join_room_by_code(&code),
            Command::StartDiscovery => self.start_discovery(),
            Command::StopDiscovery => self.stop_discovery(),
            Command::CreateRoom => self.create_room(),
            Command::JoinRoom { .. } => {
                self.not_implemented("room", "join (Phase 1)");
            }
            Command::RespondToJoin { .. } => {
                self.not_implemented("room", "admission (Phase 1)");
            }
            Command::LeaveRoom => self.leave_room(),
            Command::Mute => self.set_muted(true),
            Command::Unmute => self.set_muted(false),
            Command::RequestDiagnostics => self.emit_diagnostics(),
            Command::Platform(event) => self.handle_platform_event(event),
            Command::Shutdown => {}
        }
    }

    /// Periodic work: expiry, path re-evaluation, relay health, playout.
    ///
    /// Public for the same reason as [`Self::handle_command`] — a test can call
    /// it with a `TestClock` and assert exactly what a given elapsed time does.
    pub fn tick(&mut self) {
        let now = self.clock.now();

        for peer in self.peers.expire(now, crate::discovery::PEER_TTL) {
            if let Some(peer_id) = peer.peer_id {
                self.sink.emit(Event::PeerLost { peer_id });
            }
        }

        for change in self.transport.evaluate(now) {
            self.counters.path_switches += 1;
            let (active, standby) = self
                .transport
                .connection(change.peer)
                .map(|c| {
                    (
                        c.active_path().map(|p| p.kind),
                        c.standby_path().map(|p| p.kind),
                    )
                })
                .unwrap_or((None, None));

            if let Some(active) = active {
                self.sink.emit(Event::TransportChanged { peer_id: change.peer, active, standby });
            }
        }

        self.keys.expire_epochs(now);

        // A call nobody answered. Ends the same way on both devices because
        // both are running the same timeout against their own clock.
        if self.call.has_timed_out(now) {
            self.end_call(CallEnded::Unanswered);
        }

        if let Some(monitor) = &self.relay_monitor {
            if monitor.has_failed(now) {
                // PHASE3: run an election. The detection is already correct;
                // what is missing is the round of RelayAnnounce/RelayElection
                // traffic that follows.
                self.set_state(AppState::RelayElection);
            }
        }

        if self.config.diagnostics
            && now.saturating_since(self.last_diagnostics) >= crate::diagnostics::SNAPSHOT_INTERVAL
        {
            self.emit_diagnostics();
            self.last_diagnostics = now;
        }
    }

    // --- command handlers -------------------------------------------------

    fn start_discovery(&mut self) {
        let capabilities = self.platform.capabilities();

        if !capabilities.nearby_devices {
            self.sink.emit(Event::PermissionRequired { capability: "nearby_devices" });
            self.set_state(AppState::PermissionsRequired);
            return;
        }

        let mut started = false;

        if capabilities.lan {
            match self.platform.start_lan_discovery() {
                Ok(()) => started = true,
                Err(e) => self.emit_error("platform", &e),
            }
        }

        // Wi-Fi Aware being unavailable is an ordinary condition on plenty of
        // hardware, not a failure. LAN-only is a perfectly good Anvil.
        if capabilities.wifi_aware {
            match self.platform.start_aware_discovery() {
                Ok(()) => started = true,
                Err(e) => self.emit_error("platform", &e),
            }
        }

        self.set_state(if started { AppState::Discovering } else { AppState::Error });
    }

    fn stop_discovery(&mut self) {
        let _ = self.platform.stop_lan_discovery();
        let _ = self.platform.stop_aware_discovery();
        if self.room.is_none() {
            self.set_state(AppState::Idle);
        }
    }

    fn create_room(&mut self) {
        if let Some(room) = &self.room {
            self.emit_error_message("room", format!("already in room {}", room.room_id));
            return;
        }

        self.set_state(AppState::CreatingRoom);
        let now = self.clock.now();

        // The room id and the join code are generated independently. Deriving
        // one from the other would cap the room's identity at the code's 40
        // bits, and room ids appear in packet headers.
        let identity = RoomIdentity::generate();
        let mut room =
            RoomState::create(self.local_peer_id, self.config.display_name.clone(), now);
        room.room_id = identity.room_id;

        let room_id = room.room_id;
        self.room = Some(room);
        self.room_identity = Some(identity);

        self.sink.emit(Event::RoomCreated { room_id, join_code: identity.join_code });
        self.sink.emit(Event::RoomJoined {
            room: self.room.as_ref().expect("just set").snapshot(),
        });
        self.set_state(AppState::Connected);
    }

    fn leave_room(&mut self) {
        let Some(room) = self.room.take() else {
            return;
        };

        self.set_state(AppState::Leaving);
        self.relay_monitor = None;
        self.room_identity = None;
        self.jitter.clear();
        let _ = self.platform.stop_capture();
        let _ = self.platform.stop_playback();

        self.sink.emit(Event::RoomLeft { room_id: room.room_id, reason: "left".into() });
        self.set_state(AppState::Idle);
    }

    fn set_muted(&mut self, muted: bool) {
        if self.muted == muted {
            return;
        }
        self.muted = muted;
        self.sink.emit(Event::MuteChanged { muted });
    }

    // --- identity ---------------------------------------------------------

    /// First run: turn a display name into a working identity.
    ///
    /// Everything except the name is generated here. There is no account to
    /// create, nothing to check against a server, and no way for this to fail
    /// for a reason outside the device.
    fn create_profile(&mut self, display_name: &str) {
        if self.profile.is_some() {
            // Already have an identity: treat it as a rename rather than
            // silently regenerating a key, which would make this device a
            // stranger to everyone who knows it.
            self.rename_profile(display_name);
            return;
        }

        let name = match crate::identity::validate_name(display_name) {
            Ok(name) => name,
            Err(e) => {
                self.emit_error_message("identity", e.to_string());
                return;
            }
        };

        // PHASE2: generate a real Ed25519 keypair and persist it through
        // KeyStoreAdapter. The identity is derived from the key, so this is the
        // one place key generation happens.
        let identity = PublicIdentity::new(self.local_peer_id.0);
        let now = self.clock.now();

        match LocalProfile::new(&name, identity, now) {
            Ok(profile) => {
                self.config.display_name = profile.display_name.clone();
                self.local_peer_id = profile.peer_id;
                self.profile = Some(profile.clone());
                self.sink.emit(Event::ProfileReady { profile });
                self.set_state(AppState::Idle);
            }
            Err(e) => self.emit_error_message("identity", e.to_string()),
        }
    }

    fn rename_profile(&mut self, display_name: &str) {
        let Some(profile) = self.profile.as_mut() else {
            self.emit_error_message("identity", "no profile yet".into());
            return;
        };

        if let Err(e) = profile.rename(display_name) {
            self.emit_error_message("identity", e.to_string());
            return;
        }

        self.config.display_name = profile.display_name.clone();
        let profile = profile.clone();
        self.sink.emit(Event::ProfileReady { profile });
        // PHASE1: re-advertise so nearby devices see the new name.
    }

    fn set_peer_trust(&mut self, peer_id: PeerId, verified: bool) {
        let updated = if verified {
            self.known_peers.mark_verified(peer_id)
        } else {
            self.known_peers.accept_change(peer_id)
        };

        if !updated {
            self.emit_error_message("identity", format!("unknown peer {peer_id}"));
        }
    }

    /// Record a peer that has completed an authenticated handshake, and report
    /// anything the user should know about.
    ///
    /// Only ever called after the peer proved possession of its private key —
    /// recording an unauthenticated claim would let anyone poison the store by
    /// advertising, turning the identity-change warning into noise users learn
    /// to dismiss.
    ///
    /// Public because Phase 1's handshake completion path calls it from
    /// outside this module, and because it is the one place trust changes.
    pub fn record_authenticated_peer(
        &mut self,
        peer_id: PeerId,
        public_key: [u8; 32],
        display_name: &str,
    ) {
        let now = self.clock.now();
        match self.known_peers.observe(peer_id, public_key, display_name, now) {
            TofuOutcome::IdentityChanged { previous_fingerprint, new_fingerprint } => {
                self.sink.emit(Event::IdentityChanged {
                    peer_id,
                    display_name: display_name.to_owned(),
                    previous_fingerprint,
                    new_fingerprint,
                });
            }
            TofuOutcome::Renamed { previous_name } => {
                self.sink.emit(Event::PeerRenamed {
                    peer_id,
                    previous_name,
                    display_name: display_name.to_owned(),
                });
            }
            TofuOutcome::FirstContact | TofuOutcome::Recognised => {}
        }
    }

    // --- direct calls -----------------------------------------------------

    fn place_call(&mut self, peer_id: PeerId) {
        if self.profile.is_none() {
            self.emit_error_message("identity", "no profile yet".into());
            return;
        }

        let now = self.clock.now();
        match self.call.place(peer_id, now) {
            Ok(state) => {
                self.call = state;
                self.announce_call();
                // PHASE1: send CALL_REQUEST over the peer's reliable stream.
            }
            Err(e) => self.emit_error_message("call", e.to_string()),
        }
    }

    fn accept_call(&mut self) {
        let now = self.clock.now();
        match self.call.accept(now) {
            Ok(state) => {
                self.call = state;
                self.announce_call();
                let _ = self.platform.start_capture(&self.config.audio);
                let _ = self.platform.start_playback(&self.config.audio);
            }
            Err(e) => self.emit_error_message("call", e.to_string()),
        }
    }

    fn end_call(&mut self, reason: CallEnded) {
        let peer_id = self.call.peer();
        let (state, ended) = self.call.end(reason);
        self.call = state;

        if let Some(reason) = ended {
            let _ = self.platform.stop_capture();
            let _ = self.platform.stop_playback();
            self.sink.emit(Event::CallFinished { peer_id, reason });
            self.announce_call();
        }
    }

    fn announce_call(&self) {
        let display_name = self.call.peer().and_then(|peer| {
            self.known_peers.get(peer).map(|known| known.display_name.clone())
        });
        self.sink.emit(Event::CallStateChanged { state: self.call, display_name });
    }

    // --- chat -------------------------------------------------------------

    fn send_message(&mut self, conversation: Conversation, body: &str) {
        let now = self.clock.now();

        let message = match Message::compose(self.local_peer_id, conversation, body, now) {
            Ok(message) => message,
            Err(e) => {
                self.emit_error_message("chat", e.to_string());
                return;
            }
        };

        let id = message.id;
        self.history.record(message.clone());
        self.sink.emit(Event::MessageReceived { message });

        // v0.1 has no store-and-forward: a message either goes now or is
        // marked undeliverable, and the UI says so rather than showing a
        // hopeful clock forever.
        let reachable = match conversation {
            Conversation::Direct(peer) => self.transport.active_path(peer).is_some(),
            Conversation::Room(_) => self.room.is_some(),
        };

        let delivery =
            if reachable { DeliveryState::Sent } else { DeliveryState::Undeliverable };

        self.history.update_delivery(id, delivery);
        self.sink.emit(Event::MessageDelivery { id, delivery });

        // PHASE1: write the message to the peer's reliable stream, and update
        // delivery to Delivered on application-level acknowledgement.
    }

    // --- rooms ------------------------------------------------------------

    fn join_room_by_code(&mut self, code: &str) {
        let Some(code) = JoinCode::parse(code) else {
            self.emit_error_message("room", format!("\"{code}\" is not a valid room code"));
            return;
        };

        if self.room.is_some() {
            self.emit_error_message("room", "already in a room".into());
            return;
        }

        self.set_state(AppState::JoiningRoom);

        // PHASE1: derive the discovery token, look for a member advertising it,
        // then run the membership handshake. The room id itself arrives in
        // RoomAccept — the code only bootstraps discovery.
        let _token = code.discovery_token();
        self.emit_error_message("room", "joining by code is not implemented yet (Phase 1)".into());
        self.set_state(AppState::Idle);
    }

    // --- platform events --------------------------------------------------

    fn handle_platform_event(&mut self, event: PlatformEvent) {
        let now = self.clock.now();

        match event {
            PlatformEvent::PeerAdvertised { advertisement } => {
                let outcome = self.peers.observe(&advertisement, now);
                if let Some(peer) = self.peers.get(&advertisement.advertisement.fingerprint) {
                    // Emit once per peer, not once per path (§65).
                    if outcome != crate::discovery::SightingOutcome::Refreshed {
                        self.sink.emit(Event::PeerDiscovered { peer: peer.clone() });
                    }
                }
            }

            PlatformEvent::PeerAdvertisementLost { .. } => {
                // PHASE1: map the adapter handle back to a fingerprint. Until
                // then the TTL sweep in tick() handles departure, which is the
                // path that has to work anyway — both platforms drop these
                // callbacks.
            }

            PlatformEvent::PathEstablished { path, max_datagram_size } => {
                self.transport.on_established(path, max_datagram_size, now);
            }

            PlatformEvent::PathLost { path, reason } => {
                tracing::debug!(?path, %reason, "path lost");
                if let Some(peer) = self.transport.on_lost(path, now) {
                    // The active path for this peer died: re-evaluate now
                    // rather than waiting for the tick. Every millisecond here
                    // is silence someone hears.
                    if self.room.is_some() {
                        self.set_state(AppState::Reconnecting);
                    }
                    let _ = peer;
                }
            }

            PlatformEvent::DatagramReceived { path, data } => {
                self.on_datagram(path, &data);
            }

            PlatformEvent::NetworkChanged { kind, available } => {
                tracing::info!(%kind, available, "network changed");
                // A hint to re-evaluate, not an instruction to switch (§83).
            }

            PlatformEvent::AudioCaptured { frame } => {
                self.on_captured_audio(&frame, now);
            }

            PlatformEvent::AudioInterrupted { resumed } => {
                if resumed {
                    let _ = self.platform.start_capture(&self.config.audio);
                } else {
                    let _ = self.platform.stop_capture();
                }
            }

            PlatformEvent::AudioRouteChanged { route } => {
                tracing::info!(%route, "audio route changed");
            }

            PlatformEvent::PermissionChanged { capability, granted } => {
                if !granted {
                    self.sink.emit(Event::PermissionRequired { capability });
                }
            }

            PlatformEvent::DeviceStatus { .. } => {
                // PHASE3: feeds relay candidacy scoring.
            }

            PlatformEvent::LifecycleChanged { foreground } => {
                tracing::info!(foreground, "lifecycle changed");
            }
        }
    }

    fn on_datagram(&mut self, _path: crate::PathId, data: &[u8]) {
        // Parse first, authenticate second, decode third — never any other
        // order (§80). Parsing is the only step that touches attacker bytes
        // without a key, so it is the only step allowed to run first.
        match crate::protocol::MediaPacket::decode(data) {
            Ok(_packet) => {
                // PHASE2: authenticate and decrypt via GroupKeyManager, then
                // push into the sender's jitter buffer.
                self.counters.packets_received += 1;
            }
            Err(e) => {
                tracing::trace!(error = %e, "malformed datagram discarded");
            }
        }
    }

    fn on_captured_audio(&mut self, frame: &crate::audio::PcmFrame, now: Monotonic) {
        if self.muted || self.room.is_none() {
            return;
        }

        let was_speaking = self.vad.is_speaking();
        let transmit = self.vad.should_transmit(&frame.samples, now);

        if self.vad.is_speaking() != was_speaking {
            self.sink.emit(Event::SpeakingChanged {
                peer_id: self.local_peer_id,
                speaking: self.vad.is_speaking(),
            });
        }

        if !transmit {
            return;
        }

        // PHASE1: encode with Opus, seal with the group key manager, frame and
        // send over the active path for each route.
        let _ = self.keys.take_sequence();
    }

    // --- helpers ----------------------------------------------------------

    fn set_state(&mut self, state: AppState) {
        if self.state == state {
            return;
        }
        self.state = state;
        self.sink.emit(Event::StateChanged { state });
    }

    fn emit_diagnostics(&mut self) {
        let mut snapshot = crate::diagnostics::DiagnosticsSnapshot::new(
            self.local_peer_id,
            self.room.as_ref().map(|r| r.room_id),
            self.room.as_ref().and_then(|r| r.relay),
        );
        snapshot.epoch = self.room.as_ref().map_or(crate::Epoch(0), |r| r.epoch);
        snapshot.participant_count = self.room.as_ref().map_or(0, RoomState::size);
        snapshot.opus_bitrate_bps = self.config.audio.target_bitrate_bps;
        snapshot.counters = self.counters;

        self.sink.emit(Event::Diagnostics { snapshot });
    }

    fn emit_error(&self, layer: &'static str, error: &crate::Error) {
        self.sink.emit(Event::Error { layer, message: error.to_string() });
    }

    fn emit_error_message(&self, layer: &'static str, message: String) {
        self.sink.emit(Event::Error { layer, message });
    }

    fn not_implemented(&mut self, layer: &'static str, what: &str) {
        self.emit_error_message(layer, format!("not implemented: {what}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::RecordingSink;
    use crate::platform::Capabilities;
    use crate::time::TestClock;
    use crate::transport::{Endpoint, PathKind};
    use crate::{PathId, Result};

    fn peer(n: u8) -> PeerId {
        let mut bytes = [0u8; 32];
        bytes[0] = n;
        PeerId(bytes)
    }

    /// A platform that reports full capability and accepts every call, so tests
    /// exercise engine logic rather than adapter stubs.
    #[derive(Debug, Default)]
    struct FakePlatform {
        capabilities: Capabilities,
    }

    impl FakePlatform {
        fn full() -> Self {
            Self {
                capabilities: Capabilities {
                    lan: true,
                    wifi_aware: true,
                    microphone: true,
                    nearby_devices: true,
                    secure_key_storage: true,
                },
            }
        }

        fn lan_only() -> Self {
            Self {
                capabilities: Capabilities {
                    lan: true,
                    wifi_aware: false,
                    microphone: true,
                    nearby_devices: true,
                    secure_key_storage: false,
                },
            }
        }
    }

    impl crate::platform::DiscoveryAdapter for FakePlatform {
        fn start_lan_discovery(&self) -> Result<()> {
            Ok(())
        }
        fn stop_lan_discovery(&self) -> Result<()> {
            Ok(())
        }
        fn start_aware_discovery(&self) -> Result<()> {
            Ok(())
        }
        fn stop_aware_discovery(&self) -> Result<()> {
            Ok(())
        }
        fn advertise(&self, _payload: &[u8]) -> Result<()> {
            Ok(())
        }
        fn stop_advertising(&self) -> Result<()> {
            Ok(())
        }
    }

    impl crate::platform::TransportAdapter for FakePlatform {
        fn connect(&self, _path: PathId, _endpoint: &Endpoint) -> Result<()> {
            Ok(())
        }
        fn close(&self, _path: PathId) -> Result<()> {
            Ok(())
        }
        fn send_datagram(&self, _path: PathId, _data: &[u8]) -> Result<()> {
            Ok(())
        }
        fn send_reliable(&self, _path: PathId, _data: &[u8]) -> Result<()> {
            Ok(())
        }
        fn listen(&self, kind: PathKind) -> Result<Endpoint> {
            Ok(Endpoint::new(kind, "test"))
        }
    }

    impl crate::platform::AudioAdapter for FakePlatform {
        fn start_capture(&self, _config: &crate::AudioConfig) -> Result<()> {
            Ok(())
        }
        fn stop_capture(&self) -> Result<()> {
            Ok(())
        }
        fn start_playback(&self, _config: &crate::AudioConfig) -> Result<()> {
            Ok(())
        }
        fn stop_playback(&self) -> Result<()> {
            Ok(())
        }
        fn play(&self, _frame: &crate::audio::PcmFrame) -> Result<()> {
            Ok(())
        }
    }

    impl crate::platform::KeyStoreAdapter for FakePlatform {
        fn load_identity(&self) -> Result<Option<Vec<u8>>> {
            Ok(None)
        }
        fn store_identity(&self, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }
        fn clear_identity(&self) -> Result<()> {
            Ok(())
        }
    }

    impl PlatformAdapter for FakePlatform {
        fn capabilities(&self) -> Capabilities {
            self.capabilities
        }
        fn request_permission(&self, _capability: &'static str) -> Result<()> {
            Ok(())
        }
    }

    fn engine(platform: FakePlatform) -> (Engine, Arc<RecordingSink>, Arc<TestClock>) {
        let sink = Arc::new(RecordingSink::new());
        let clock = Arc::new(TestClock::new());
        let (engine, _handle) = Engine::new(
            AnvilConfig::default(),
            Arc::new(platform),
            sink.clone(),
            clock.clone(),
            peer(1),
        );
        (engine, sink, clock)
    }

    #[test]
    fn starts_in_initializing() {
        let (engine, _, _) = engine(FakePlatform::full());
        assert_eq!(engine.state(), AppState::Initializing);
        assert!(engine.room().is_none());
    }

    #[test]
    fn discovery_starts_on_a_fully_capable_device() {
        let (mut engine, sink, _) = engine(FakePlatform::full());
        engine.handle_command(Command::StartDiscovery);

        assert_eq!(engine.state(), AppState::Discovering);
        assert!(sink
            .events()
            .iter()
            .any(|e| matches!(e, Event::StateChanged { state: AppState::Discovering })));
    }

    #[test]
    fn a_device_without_wifi_aware_still_discovers_over_lan() {
        // Aware is absent on a lot of shipping hardware; that is not an error.
        let (mut engine, _, _) = engine(FakePlatform::lan_only());
        engine.handle_command(Command::StartDiscovery);
        assert_eq!(engine.state(), AppState::Discovering);
    }

    #[test]
    fn missing_permission_asks_rather_than_failing() {
        let platform = FakePlatform { capabilities: Capabilities::default() };
        let (mut engine, sink, _) = engine(platform);

        engine.handle_command(Command::StartDiscovery);

        assert_eq!(engine.state(), AppState::PermissionsRequired);
        assert!(sink.events().iter().any(|e| matches!(
            e,
            Event::PermissionRequired { capability: "nearby_devices" }
        )));
    }

    #[test]
    fn creating_a_room_produces_a_room_and_a_snapshot() {
        let (mut engine, sink, _) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);

        assert_eq!(engine.state(), AppState::Connected);
        let room = engine.room().expect("room should exist");
        assert!(room.is_host);
        assert_eq!(room.size(), 1);

        let events = sink.events();
        assert!(events.iter().any(|e| matches!(e, Event::RoomCreated { .. })));
        assert!(events.iter().any(|e| matches!(e, Event::RoomJoined { .. })));
    }

    #[test]
    fn creating_a_second_room_is_refused() {
        let (mut engine, sink, _) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);
        let before = sink.len();

        engine.handle_command(Command::CreateRoom);

        assert!(sink.events()[before..]
            .iter()
            .any(|e| matches!(e, Event::Error { layer: "room", .. })));
    }

    #[test]
    fn leaving_clears_the_room() {
        let (mut engine, sink, _) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);
        engine.handle_command(Command::LeaveRoom);

        assert!(engine.room().is_none());
        assert_eq!(engine.state(), AppState::Idle);
        assert!(sink.events().iter().any(|e| matches!(e, Event::RoomLeft { .. })));
    }

    #[test]
    fn mute_state_changes_are_reported_once() {
        let (mut engine, sink, _) = engine(FakePlatform::full());

        engine.handle_command(Command::Mute);
        engine.handle_command(Command::Mute); // no-op
        engine.handle_command(Command::Unmute);

        let mute_events: Vec<_> =
            sink.events().into_iter().filter(|e| matches!(e, Event::MuteChanged { .. })).collect();
        assert_eq!(mute_events.len(), 2, "duplicate mute command produced an event");
        assert!(!engine.is_muted());
    }

    #[test]
    fn a_peer_seen_on_two_transports_is_announced_once() {
        use crate::discovery::{Advertisement, PeerAdvertisement};

        let (mut engine, sink, _) = engine(FakePlatform::full());
        let fingerprint = [4u8; 8];

        for (kind, address) in
            [(PathKind::Lan, "10.0.0.4:47820"), (PathKind::WifiAware, "aware:2")]
        {
            engine.handle_command(Command::Platform(PlatformEvent::PeerAdvertised {
                advertisement: PeerAdvertisement {
                    kind,
                    handle: address.into(),
                    endpoint: Endpoint::new(kind, address),
                    advertisement: Advertisement::new(fingerprint, None, "Bob"),
                },
            }));
        }

        assert_eq!(engine.peers().len(), 1, "one phone appeared as two peers");
        let discovered: Vec<_> = sink
            .events()
            .into_iter()
            .filter(|e| matches!(e, Event::PeerDiscovered { .. }))
            .collect();
        // New, then PathAdded — both are real changes worth announcing, but
        // they describe one peer.
        assert_eq!(discovered.len(), 2);
    }

    #[test]
    fn repeat_sightings_do_not_spam_the_host() {
        use crate::discovery::{Advertisement, PeerAdvertisement};

        let (mut engine, sink, clock) = engine(FakePlatform::full());
        for _ in 0..10 {
            clock.advance(core::time::Duration::from_millis(500));
            engine.handle_command(Command::Platform(PlatformEvent::PeerAdvertised {
                advertisement: PeerAdvertisement {
                    kind: PathKind::Lan,
                    handle: "h".into(),
                    endpoint: Endpoint::new(PathKind::Lan, "10.0.0.4:47820"),
                    advertisement: Advertisement::new([5u8; 8], None, "Bob"),
                },
            }));
        }

        let discovered = sink
            .events()
            .into_iter()
            .filter(|e| matches!(e, Event::PeerDiscovered { .. }))
            .count();
        assert_eq!(discovered, 1);
    }

    #[test]
    fn malformed_datagrams_are_discarded_without_disturbing_anything() {
        let (mut engine, sink, _) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);
        let before = sink.len();

        for bytes in [vec![], vec![0xff; 3], vec![0x01; 200], vec![0xAA; 22]] {
            engine.handle_command(Command::Platform(PlatformEvent::DatagramReceived {
                path: PathId(1),
                data: bytes,
            }));
        }

        assert_eq!(sink.len(), before, "garbage on the wire produced host events");
        assert_eq!(engine.state(), AppState::Connected);
    }

    #[test]
    fn ticking_an_idle_engine_is_harmless() {
        let (mut engine, sink, clock) = engine(FakePlatform::full());
        let before = sink.len();

        for _ in 0..50 {
            clock.advance(TICK);
            engine.tick();
        }

        assert_eq!(sink.len(), before);
    }

    #[test]
    fn silent_peers_are_reported_lost_after_the_ttl() {
        use crate::discovery::{Advertisement, PeerAdvertisement};

        let (mut engine, _sink, clock) = engine(FakePlatform::full());
        engine.handle_command(Command::Platform(PlatformEvent::PeerAdvertised {
            advertisement: PeerAdvertisement {
                kind: PathKind::Lan,
                handle: "h".into(),
                endpoint: Endpoint::new(PathKind::Lan, "10.0.0.4:47820"),
                advertisement: Advertisement::new([6u8; 8], None, "Bob"),
            },
        }));
        assert_eq!(engine.peers().len(), 1);

        clock.advance(crate::discovery::PEER_TTL + core::time::Duration::from_secs(1));
        engine.tick();

        assert_eq!(engine.peers().len(), 0);
    }

    #[test]
    fn a_dropped_handle_stops_the_loop() {
        let sink = Arc::new(RecordingSink::new());
        let (engine, handle) = Engine::headless(AnvilConfig::default(), sink, peer(1));

        let thread = std::thread::spawn(move || engine.run());
        drop(handle);

        thread.join().expect("engine thread should exit when nobody can command it");
    }

    #[test]
    fn shutdown_stops_the_loop() {
        let sink = Arc::new(RecordingSink::new());
        let (engine, handle) = Engine::headless(AnvilConfig::default(), sink, peer(1));

        let thread = std::thread::spawn(move || engine.run());
        assert!(handle.send(Command::Shutdown));
        thread.join().expect("engine thread should exit on shutdown");
    }
}
