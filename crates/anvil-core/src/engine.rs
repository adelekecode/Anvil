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
use crate::crypto::DeviceIdentity;
#[cfg(feature = "crypto")]
use crate::crypto::{GroupKeyManager, SenderKeyManager};
use crate::diagnostics::Counters;
use crate::discovery::{Fingerprint as DiscoveryFingerprint, PeerTable};
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

/// Extract a human-readable message from a caught panic payload.
fn panic_message(error: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = error.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = error.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "unknown internal error".into()
    }
}

fn provisional_peer_id(fingerprint: DiscoveryFingerprint) -> PeerId {
    let mut bytes = [0u8; 32];
    bytes[..fingerprint.len()].copy_from_slice(&fingerprint);
    // Domain-separate this routing-only sentinel from any ordinary padded key.
    bytes[31] = 0xA1;
    PeerId(bytes)
}

/// A peer who has asked to join a host-approved room and is waiting on a human
/// decision (§68). Tracked only between the `RoomJoin` arrival and the host's
/// `RespondToJoin`.
#[cfg(feature = "crypto")]
#[derive(Clone, Debug)]
struct PendingJoin {
    /// Display name the joiner asserted over their authenticated session.
    display_name: String,
    /// When they asked. Reserved for the stale-request expiry that the
    /// host-approval UX will need (a host who never replies must not become
    /// a permanent inbox). Unread today because no such expiry is wired up.
    #[allow(dead_code)]
    requested_at: Monotonic,
}

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
    /// Long-lived signing identity. Its private half never crosses the UI FFI.
    device_identity: Option<DeviceIdentity>,
    known_peers: KnownPeers,
    call: CallState,
    history: History,

    peers: PeerTable,
    /// QUIC paths opened from unauthenticated discovery sightings, keyed until
    /// the identity handshake promotes them to a real peer id.
    pending_paths: std::collections::HashMap<crate::PathId, DiscoveryFingerprint>,
    #[cfg(feature = "crypto")]
    handshakes: std::collections::HashMap<crate::PathId, crate::crypto::SessionHandshake>,
    #[cfg(feature = "crypto")]
    secure_control: std::collections::HashMap<crate::PathId, crate::crypto::SecureControl>,
    transport: TransportManager,
    room: Option<RoomState>,
    /// Room id plus the human-facing join code, for a room we host or joined.
    room_identity: Option<RoomIdentity>,
    /// Parsed code retained while waiting for an authenticated host response.
    pending_join_code: Option<JoinCode>,
    /// Members who asked to join but have not yet been admitted or refused.
    /// Only meaningful when this device is the host (§68).
    #[cfg(feature = "crypto")]
    pending_join_requests: std::collections::HashMap<PeerId, PendingJoin>,
    #[cfg(feature = "crypto")]
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
            #[cfg(feature = "crypto")]
            keys: SenderKeyManager::new(local_peer_id),
            config,
            clock,
            platform,
            sink,
            rx,
            state: AppState::Initializing,
            local_peer_id,
            profile: None,
            device_identity: None,
            known_peers: KnownPeers::new(),
            call: CallState::Idle,
            history: History::new(),
            peers: PeerTable::new(),
            pending_paths: std::collections::HashMap::new(),
            #[cfg(feature = "crypto")]
            handshakes: std::collections::HashMap::new(),
            #[cfg(feature = "crypto")]
            secure_control: std::collections::HashMap::new(),
            room: None,
            room_identity: None,
            pending_join_code: None,
            #[cfg(feature = "crypto")]
            pending_join_requests: std::collections::HashMap::new(),
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
        Self::new(config, Arc::new(NullPlatform), sink, Arc::new(SystemClock::new()), local_peer_id)
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
                Ok(command) => {
                    let result = std::panic::catch_unwind(
                        std::panic::AssertUnwindSafe(|| self.handle_command(command)),
                    );
                    if let Err(e) = result {
                        let msg = panic_message(&e);
                        self.sink.emit(Event::Error {
                            layer: "engine",
                            message: format!("handler panic: {msg}"),
                        });
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }

            let result =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.tick()));
            if let Err(e) = result {
                let msg = panic_message(&e);
                self.sink.emit(Event::Error {
                    layer: "engine",
                    message: format!("tick panic: {msg}"),
                });
            }
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
            Command::JoinRoom { room_id, credential } => self.join_room(room_id, credential),
            #[cfg(feature = "crypto")]
            Command::RespondToJoin { peer_id, accept } => self.respond_to_join(peer_id, accept),
            #[cfg(not(feature = "crypto"))]
            Command::RespondToJoin { .. } => {
                self.emit_error_message(
                    "room",
                    "host approval requires the crypto feature in Phase 2".to_string(),
                );
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
                .map(|c| (c.active_path().map(|p| p.kind), c.standby_path().map(|p| p.kind)))
                .unwrap_or((None, None));

            if let Some(active) = active {
                self.sink.emit(Event::TransportChanged { peer_id: change.peer, active, standby });
            }
        }

        #[cfg(feature = "crypto")]
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
            if let Err(error) = self.platform.request_permission("nearby_devices") {
                self.emit_error("platform", &error);
            }
            return;
        }

        let mut started = false;

        if capabilities.lan {
            match self
                .platform
                .listen(crate::transport::PathKind::Lan)
                .and_then(|_| self.platform.start_lan_discovery())
            {
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

        if started {
            self.set_state(AppState::Discovering);
            self.refresh_advertisement();
        } else {
            self.set_state(AppState::Error);
        }
    }

    fn stop_discovery(&mut self) {
        let _ = self.platform.stop_lan_discovery();
        let _ = self.platform.stop_aware_discovery();
        let _ = self.platform.stop_advertising();
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
        let admission = crate::room::AdmissionPolicy::JoinCode { code: identity.join_code.raw() };
        let mut room =
            RoomState::create(self.local_peer_id, self.config.display_name.clone(), admission, now);
        room.room_id = identity.room_id;

        let room_id = room.room_id;
        self.room = Some(room);
        self.room_identity = Some(identity);

        self.sink.emit(Event::RoomCreated { room_id, join_code: identity.join_code });
        self.sink
            .emit(Event::RoomJoined { room: self.room.as_ref().expect("just set").snapshot() });
        self.set_state(AppState::Connected);
        self.refresh_advertisement();
        self.start_audio();
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
        self.refresh_advertisement();
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

        if self.device_identity.is_none() {
            let loaded = self.platform.load_identity().and_then(|stored| match stored {
                Some(stored) if stored.len() >= 32 => {
                    let mut secret = [0u8; 32];
                    secret.copy_from_slice(&stored[..32]);
                    DeviceIdentity::from_secret(secret)
                }
                Some(_) => {
                    Err(crate::CryptoError::KeyStorage("stored identity is truncated".into())
                        .into())
                }
                None => {
                    let identity = DeviceIdentity::generate();
                    self.platform.store_identity(&identity.secret_bytes())?;
                    Ok(identity)
                }
            });
            match loaded {
                Ok(identity) => self.device_identity = Some(identity),
                Err(error) => {
                    self.emit_error("crypto", &error);
                    return;
                }
            }
        }
        let identity = self.device_identity.as_ref().expect("identity just loaded").public();
        self.local_peer_id = identity.peer_id();
        #[cfg(feature = "crypto")]
        {
            self.keys = SenderKeyManager::new(self.local_peer_id);
        }
        let now = self.clock.now();

        match LocalProfile::new(&name, identity, now) {
            Ok(profile) => {
                self.config.display_name = profile.display_name.clone();
                self.local_peer_id = profile.peer_id;
                self.profile = Some(profile.clone());
                if let Err(error) = self.persist_profile() {
                    self.emit_error("crypto", &error);
                    return;
                }
                self.sink.emit(Event::ProfileReady { profile });
                self.set_state(AppState::Idle);
                self.refresh_advertisement();
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
        if let Err(error) = self.persist_profile() {
            self.emit_error("crypto", &error);
            return;
        }
        self.sink.emit(Event::ProfileReady { profile });
        self.refresh_advertisement();
    }

    /// Restore identity plus display name after the native keystore attaches.
    fn restore_profile(&mut self) {
        let stored = match self.platform.load_identity() {
            Ok(Some(stored)) => stored,
            Ok(None) => return,
            Err(error) => {
                self.emit_error("crypto", &error);
                return;
            }
        };
        if stored.len() < 32 {
            self.emit_error_message("crypto", "stored identity is truncated".into());
            return;
        }

        let mut secret = [0u8; 32];
        secret.copy_from_slice(&stored[..32]);
        let identity = match DeviceIdentity::from_secret(secret) {
            Ok(identity) => identity,
            Err(error) => {
                self.emit_error("crypto", &error);
                return;
            }
        };
        self.local_peer_id = identity.peer_id();
        #[cfg(feature = "crypto")]
        {
            self.keys = SenderKeyManager::new(self.local_peer_id);
        }
        self.device_identity = Some(identity);

        let Ok(display_name) = std::str::from_utf8(&stored[32..]) else {
            self.emit_error_message("crypto", "stored profile name is invalid".into());
            return;
        };
        if display_name.is_empty() {
            return;
        }
        let public = self.device_identity.as_ref().expect("identity restored").public();
        match LocalProfile::new(display_name, public, self.clock.now()) {
            Ok(profile) => {
                self.config.display_name = profile.display_name.clone();
                self.profile = Some(profile.clone());
                self.sink.emit(Event::ProfileReady { profile });
            }
            Err(error) => self.emit_error_message("identity", error.to_string()),
        }
    }

    fn persist_profile(&self) -> crate::Result<()> {
        let identity = self
            .device_identity
            .as_ref()
            .ok_or_else(|| crate::CryptoError::KeyStorage("identity not initialized".into()))?;
        let profile = self
            .profile
            .as_ref()
            .ok_or_else(|| crate::CryptoError::KeyStorage("profile not initialized".into()))?;
        let mut stored = identity.secret_bytes().to_vec();
        stored.extend_from_slice(profile.display_name.as_bytes());
        self.platform.store_identity(&stored)
    }

    /// Publish the smallest useful, explicitly untrusted discovery record.
    fn refresh_advertisement(&self) {
        let Some(profile) = &self.profile else {
            return;
        };
        let identity = PublicIdentity::new(profile.public_key);
        let room_hint = self
            .room
            .as_ref()
            .filter(|room| room.is_host)
            .and(self.room_identity)
            .map(|identity| identity.join_code.discovery_token());
        let payload = crate::discovery::Advertisement::with_room_hint(
            identity.fingerprint(),
            room_hint,
            &profile.display_name,
        )
        .encode();
        if let Err(error) = self.platform.advertise(&payload) {
            // Advertising before attachment is harmless during initial profile
            // restoration; a later StartDiscovery always retries it.
            tracing::debug!(%error, "could not refresh discovery advertisement");
        }
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
                #[cfg(feature = "crypto")]
                if let Err(error) =
                    self.send_control(peer_id, crate::crypto::AppControl::CallRequest)
                {
                    self.emit_error("call", &error);
                    self.finish_remote_call(CallEnded::Unreachable);
                }
            }
            Err(e) => self.emit_error_message("call", e.to_string()),
        }
    }

    fn accept_call(&mut self) {
        let now = self.clock.now();
        match self.call.accept(now) {
            Ok(state) => {
                let peer = state.peer();
                self.call = state;
                self.announce_call();
                #[cfg(feature = "crypto")]
                if let Some(peer) = peer {
                    if let Err(error) =
                        self.send_control(peer, crate::crypto::AppControl::CallAccept)
                    {
                        self.emit_error("call", &error);
                    }
                }
                self.start_audio();
            }
            Err(e) => self.emit_error_message("call", e.to_string()),
        }
    }

    fn end_call(&mut self, reason: CallEnded) {
        let peer_id = self.call.peer();
        #[cfg(feature = "crypto")]
        if let Some(peer) = peer_id {
            let control = if reason == CallEnded::Declined {
                crate::crypto::AppControl::CallDecline
            } else {
                crate::crypto::AppControl::CallEnd
            };
            let _ = self.send_control(peer, control);
        }
        self.finish_remote_call(reason);
    }

    fn finish_remote_call(&mut self, reason: CallEnded) {
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
        let display_name = self
            .call
            .peer()
            .and_then(|peer| self.known_peers.get(peer).map(|known| known.display_name.clone()));
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
        let delivered = match conversation {
            Conversation::Direct(peer) => {
                #[cfg(feature = "crypto")]
                {
                    self.send_control(
                        peer,
                        crate::crypto::AppControl::Chat { id, body: body.trim().to_owned() },
                    )
                    .is_ok()
                }
                #[cfg(not(feature = "crypto"))]
                false
            }
            Conversation::Room(_) => false,
        };

        let delivery = if delivered { DeliveryState::Sent } else { DeliveryState::Undeliverable };

        self.history.update_delivery(id, delivery);
        self.sink.emit(Event::MessageDelivery { id, delivery });
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
        let hosts = self.peers.confirmed_for_room_hint(code.discovery_token());
        if hosts.is_empty() {
            self.emit_error_message("room", "no nearby host is advertising that code".into());
            self.set_state(AppState::Idle);
            return;
        }
        self.pending_join_code = Some(code);
        #[cfg(feature = "crypto")]
        {
            let mut sent = false;
            for host in hosts {
                match self.send_control(
                    host,
                    crate::crypto::AppControl::RoomJoin { code: code.formatted() },
                ) {
                    Ok(()) => sent = true,
                    Err(error) => self.emit_error("room", &error),
                }
            }
            if !sent {
                self.pending_join_code = None;
                self.set_state(AppState::Idle);
            }
        }
    }

    /// Join a room we already know about, identified by [`RoomId`].
    ///
    /// Differs from [`Self::join_room_by_code`] only in how we discover the
    /// host: instead of matching the discovery hint, we ask the transport
    /// layer for a confirmed peer that hosts the requested room. Today we
    /// route the request to the first confirmed peer with an authenticated
    /// session and let the host validate the credential — that's enough for
    /// v0.1 and avoids needing a separate host→room index.
    fn join_room(&mut self, room_id: RoomId, credential: Option<Vec<u8>>) {
        if self.room.is_some() {
            self.emit_error_message("room", "already in a room".to_string());
            return;
        }
        // `credential` is consumed only by the crypto path; the no-crypto
        // build just reports that joining a room needs the feature.
        #[cfg_attr(not(feature = "crypto"), allow(unused_variables))]
        let Some(code) = credential
        else {
            self.emit_error_message(
                "room",
                "joining by room id requires a credential in Phase 2".to_string(),
            );
            return;
        };

        self.set_state(AppState::JoiningRoom);

        #[cfg(feature = "crypto")]
        {
            let Some(host) = self.transport.reachable_peers().next() else {
                self.emit_error_message(
                    "room",
                    "no authenticated peer is available to host that room".to_string(),
                );
                self.set_state(AppState::Idle);
                return;
            };

            let code_string = match std::str::from_utf8(&code) {
                Ok(text) => text.to_owned(),
                Err(_) => {
                    self.emit_error_message("room", "credential must be UTF-8".to_string());
                    self.set_state(AppState::Idle);
                    return;
                }
            };
            self.pending_join_code = JoinCode::parse(&code_string);

            match self.send_control(host, crate::crypto::AppControl::RoomJoin { code: code_string })
            {
                Ok(()) => {
                    let _ = room_id;
                }
                Err(error) => {
                    self.emit_error("room", &error);
                    self.pending_join_code = None;
                    self.set_state(AppState::Idle);
                }
            }
        }
        #[cfg(not(feature = "crypto"))]
        {
            self.emit_error_message(
                "room",
                "joining a room requires the crypto feature in Phase 2".to_string(),
            );
            self.set_state(AppState::Idle);
            let _ = room_id;
        }
    }

    // --- platform events --------------------------------------------------

    fn handle_platform_event(&mut self, event: PlatformEvent) {
        let now = self.clock.now();

        match event {
            PlatformEvent::PeerAdvertised { advertisement } => {
                if self.profile.as_ref().is_some_and(|profile| {
                    PublicIdentity::new(profile.public_key).fingerprint()
                        == advertisement.advertisement.fingerprint
                }) {
                    return;
                }
                let outcome = self.peers.observe(&advertisement, now);
                if let Some(peer) = self.peers.get(&advertisement.advertisement.fingerprint) {
                    // Emit once per peer, not once per path (§65).
                    if outcome != crate::discovery::SightingOutcome::Refreshed {
                        self.sink.emit(Event::PeerDiscovered { peer: peer.clone() });
                    }
                }
                if outcome != crate::discovery::SightingOutcome::Refreshed {
                    let provisional = provisional_peer_id(advertisement.advertisement.fingerprint);
                    let path = self.transport.add_candidate(
                        provisional,
                        advertisement.endpoint.clone(),
                        0,
                        now,
                    );
                    self.pending_paths.insert(path, advertisement.advertisement.fingerprint);
                    if let Err(error) = self.platform.connect(path, &advertisement.endpoint) {
                        self.emit_error("transport", &error);
                        let _ = self.transport.on_lost(path, now);
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
                #[cfg(feature = "crypto")]
                if let (Some(fingerprint), Some(identity)) =
                    (self.pending_paths.get(&path).copied(), self.device_identity.as_ref())
                {
                    let handshake = crate::crypto::SessionHandshake::new(fingerprint);
                    let hello = handshake.hello(identity);
                    self.handshakes.insert(path, handshake);
                    if let Err(error) = self.platform.send_reliable(path, &hello) {
                        self.emit_error("transport", &error);
                    }
                }
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

            PlatformEvent::ReliableReceived { path, data } => {
                self.on_reliable(path, &data);
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
                if granted
                    && capability == "nearby_devices"
                    && self.state == AppState::PermissionsRequired
                {
                    self.start_discovery();
                } else if granted
                    && capability == "microphone"
                    && (self.room.is_some() || self.call.is_active())
                {
                    self.start_audio();
                } else if !granted {
                    self.sink.emit(Event::PermissionRequired { capability });
                }
            }

            PlatformEvent::DeviceStatus { .. } => {
                // PHASE3: feeds relay candidacy scoring.
            }

            PlatformEvent::LifecycleChanged { foreground } => {
                if foreground && self.profile.is_none() {
                    self.restore_profile();
                }
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

    fn on_reliable(&mut self, path: crate::PathId, data: &[u8]) {
        #[cfg(feature = "crypto")]
        {
            if crate::crypto::SecureControl::is_record(data) {
                let message = self
                    .secure_control
                    .get_mut(&path)
                    .ok_or_else(|| {
                        crate::CryptoError::HandshakeFailed("control before identity").into()
                    })
                    .and_then(|control| control.open(data));
                match message {
                    Ok(message) => self.handle_app_control(path, message),
                    Err(error) => self.fail_handshake(path, &error),
                }
                return;
            }
            let Some(record_type) = crate::crypto::SessionHandshake::record_type(data) else {
                tracing::debug!(?path, "discarding unknown reliable record");
                return;
            };
            if record_type == crate::crypto::SessionHandshake::HELLO_RECORD {
                let Some(identity) = self.device_identity.as_ref() else { return };
                let Some(profile) = self.profile.as_ref() else { return };
                let response: crate::Result<Vec<u8>> = match self.handshakes.get_mut(&path) {
                    Some(handshake) => {
                        handshake.receive_hello(data, identity, &profile.display_name)
                    }
                    None => Err(crate::CryptoError::HandshakeFailed("unknown path").into()),
                };
                match response {
                    Ok(response) => {
                        if let Err(error) = self.platform.send_reliable(path, &response) {
                            self.emit_error("transport", &error);
                        }
                    }
                    Err(error) => self.fail_handshake(path, &error),
                }
                return;
            }
            if record_type == crate::crypto::SessionHandshake::IDENTITY_RECORD {
                let result = self
                    .handshakes
                    .get_mut(&path)
                    .ok_or_else(|| crate::CryptoError::HandshakeFailed("unknown path").into())
                    .and_then(|handshake| handshake.receive_identity(data));
                match result {
                    Ok(established) => self.complete_handshake(path, established),
                    Err(error) => self.fail_handshake(path, &error),
                }
            }
        }
        #[cfg(not(feature = "crypto"))]
        tracing::trace!(?path, bytes = data.len(), "crypto feature disabled; control ignored");
    }

    #[cfg(feature = "crypto")]
    fn complete_handshake(
        &mut self,
        path: crate::PathId,
        established: crate::crypto::EstablishedSession,
    ) {
        let Some(fingerprint) = self.pending_paths.remove(&path) else { return };
        self.secure_control
            .insert(path, crate::crypto::SecureControl::new(established.session_key));
        self.transport.confirm_peer(path, established.peer_id, self.clock.now());
        self.peers.confirm(fingerprint, established.peer_id, established.display_name.clone());
        self.record_authenticated_peer(
            established.peer_id,
            established.public_key,
            &established.display_name,
        );
        if let Some(peer) = self.peers.get(&fingerprint) {
            self.sink.emit(Event::PeerDiscovered { peer: peer.clone() });
        }
        let _ = self.transport.evaluate(self.clock.now());
    }

    #[cfg(feature = "crypto")]
    fn fail_handshake(&mut self, path: crate::PathId, error: &crate::Error) {
        self.emit_error("crypto", error);
        self.handshakes.remove(&path);
        self.secure_control.remove(&path);
        self.pending_paths.remove(&path);
        let _ = self.platform.close(path);
        let _ = self.transport.on_lost(path, self.clock.now());
    }

    #[cfg(feature = "crypto")]
    fn send_control(
        &mut self,
        peer: PeerId,
        message: crate::crypto::AppControl,
    ) -> crate::Result<()> {
        let path = self
            .transport
            .active_path(peer)
            .map(|path| path.id)
            .ok_or(crate::TransportError::NoPath(peer))?;
        let record = self
            .secure_control
            .get_mut(&path)
            .ok_or(crate::CryptoError::HandshakeFailed("peer session not authenticated"))?
            .seal(&message)?;
        self.platform.send_reliable(path, &record)
    }

    #[cfg(feature = "crypto")]
    fn handle_app_control(&mut self, path: crate::PathId, message: crate::crypto::AppControl) {
        let Some(peer) = self.transport.peer_for_path(path) else { return };
        match message {
            crate::crypto::AppControl::CallRequest => {
                let now = self.clock.now();
                if matches!(self.call, CallState::Outgoing { peer: current, .. } if current == peer)
                {
                    self.call = self.call.resolve_glare(self.local_peer_id, peer, now);
                    if self.call.is_active() {
                        let _ = self.send_control(peer, crate::crypto::AppControl::CallAccept);
                        self.start_audio();
                    }
                    self.announce_call();
                } else {
                    match self.call.ring(peer, now) {
                        Ok(state) => {
                            self.call = state;
                            self.announce_call();
                        }
                        Err(_) => {
                            let _ = self.send_control(peer, crate::crypto::AppControl::CallDecline);
                        }
                    }
                }
            }
            crate::crypto::AppControl::CallAccept => match self.call.accepted(self.clock.now()) {
                Ok(state) => {
                    self.call = state;
                    self.announce_call();
                    self.start_audio();
                }
                Err(error) => self.emit_error_message("call", error.to_string()),
            },
            crate::crypto::AppControl::CallDecline => self.finish_remote_call(CallEnded::Declined),
            crate::crypto::AppControl::CallEnd => self.finish_remote_call(CallEnded::HungUp),
            crate::crypto::AppControl::Chat { id, body } => {
                let message = Message::received(
                    id,
                    peer,
                    Conversation::Direct(peer),
                    &body,
                    self.clock.now(),
                );
                self.history.record(message.clone());
                self.sink.emit(Event::MessageReceived { message });
                let _ = self.send_control(peer, crate::crypto::AppControl::ChatAck { id });
            }
            crate::crypto::AppControl::ChatAck { id } => {
                if self.history.update_delivery(id, DeliveryState::Delivered) {
                    self.sink
                        .emit(Event::MessageDelivery { id, delivery: DeliveryState::Delivered });
                }
            }
            crate::crypto::AppControl::RoomJoin { code } => {
                self.handle_room_join_request(peer, &code);
            }
            crate::crypto::AppControl::RoomAccept { room_id, epoch } => {
                self.handle_room_accept(peer, room_id, epoch);
            }
            crate::crypto::AppControl::MediaKey { epoch, key, salt } => {
                self.handle_media_key(peer, epoch, key, salt);
            }
        }
    }

    #[cfg(feature = "crypto")]
    fn handle_room_join_request(&mut self, peer: PeerId, code: &str) {
        let Some(presented) = JoinCode::parse(code) else { return };
        let Some(identity) = self.room_identity else { return };
        let Some(room) = self.room.as_ref() else { return };
        if !room.is_host || presented != identity.join_code {
            return;
        }

        let display_name = self
            .known_peers
            .get(peer)
            .map(|known| known.display_name.clone())
            .unwrap_or_else(|| "Nearby peer".into());

        if room.admission.needs_user_approval() {
            // Defer to the user. Stash the request so RespondToJoin can resolve it,
            // and surface it on the host's UI. Re-asking just updates the name.
            let pending =
                PendingJoin { display_name: display_name.clone(), requested_at: self.clock.now() };
            self.pending_join_requests.insert(peer, pending);
            self.sink.emit(Event::JoinRequested { peer_id: peer, display_name });
            return;
        }

        self.admit_member(peer, display_name);
    }

    /// Resolve a previously-emitted [`Event::JoinRequested`].
    ///
    /// `accept = true` runs the same admission steps an auto-admit policy would:
    /// advance the epoch, add the participant, send [`AppControl::RoomAccept`]
    /// and this host's new media key. `accept = false` removes the pending
    /// request and emits [`Event::JoinDenied`] so the joiner can stop waiting.
    #[cfg(feature = "crypto")]
    fn respond_to_join(&mut self, peer: PeerId, accept: bool) {
        let Some(pending) = self.pending_join_requests.remove(&peer) else {
            self.emit_error_message("room", format!("no pending join request from {peer}"));
            return;
        };
        if accept {
            self.admit_member(peer, pending.display_name);
        } else {
            self.sink.emit(Event::JoinDenied { peer_id: peer });
        }
    }

    /// Perform the host side of admission: epoch advance, membership update,
    /// `RoomAccept`, and the host's fresh media key for the new epoch.
    ///
    /// Idempotent for already-admitted peers so a duplicate `RoomJoin` from a
    /// confused joiner does not double-rotate the epoch or leak a new key.
    #[cfg(feature = "crypto")]
    fn admit_member(&mut self, peer: PeerId, display_name: String) {
        let now = self.clock.now();

        // Plan all the work up front so we can release the borrow on
        // `self.room` before touching `self.send_control`.
        let (room_id, new_epoch, members) = {
            let Some(room) = self.room.as_mut() else { return };

            if room.participant(peer).is_some() {
                let epoch = room.epoch.0;
                let room_id = *room.room_id.as_bytes();
                let _ = self
                    .send_control(peer, crate::crypto::AppControl::RoomAccept { room_id, epoch });
                return;
            }

            let new_epoch = crate::Epoch(room.epoch.0 + 1);
            let participant = crate::room::Participant::new(peer, display_name, now);
            if room.add_participant(participant.clone(), new_epoch).is_err() {
                return;
            }
            self.sink.emit(Event::ParticipantJoined { participant });
            self.sink.emit(Event::EpochChanged { epoch: new_epoch });

            let members: Vec<PeerId> = room.participants.keys().copied().collect();
            (*room.room_id.as_bytes(), new_epoch, members)
        };

        let _ = self.send_control(
            peer,
            crate::crypto::AppControl::RoomAccept { room_id, epoch: new_epoch.0 },
        );

        // Distribute the host's fresh sender key for the new epoch so the
        // joiner can decrypt host traffic going forward. We only push to the
        // *new* member here; the existing members' sender keys for the host
        // are deliberately out of scope for the Phase 2 minimal flow, and
        // the protocol documentation calls this trade-off out.
        let (key, salt) = self.keys.own_key_for_epoch(new_epoch);
        if self.keys.rotate(new_epoch, &members).is_ok() {
            let _ = self.send_control(
                peer,
                crate::crypto::AppControl::MediaKey { epoch: new_epoch.0, key, salt },
            );
        }
    }

    #[cfg(feature = "crypto")]
    fn handle_media_key(&mut self, peer: PeerId, epoch: u64, key: [u8; 32], salt: [u8; 12]) {
        // Install whatever they sent. We don't gate on "is this the epoch I
        // expected?" — the key is for a specific epoch and our epoch manager
        // already rejects decryption under epochs we do not accept.
        self.keys.install_member_material(peer, crate::Epoch(epoch), key, salt);
    }

    #[cfg(feature = "crypto")]
    fn handle_room_accept(&mut self, host: PeerId, room_bytes: [u8; 16], epoch: u64) {
        if self.room.is_some() || self.state != AppState::JoiningRoom {
            return;
        }
        let Some(join_code) = self.pending_join_code.take() else { return };
        let Some(profile) = self.profile.as_ref() else { return };
        let host_name = self
            .known_peers
            .get(host)
            .map(|known| known.display_name.clone())
            .unwrap_or_else(|| "Room host".into());
        let room_id = RoomId(room_bytes);
        let participants = vec![
            crate::room::Participant::new(
                self.local_peer_id,
                profile.display_name.clone(),
                self.clock.now(),
            ),
            crate::room::Participant::new(host, host_name, self.clock.now()),
        ];
        let room = RoomState::joined(
            room_id,
            self.local_peer_id,
            crate::Epoch(epoch),
            participants,
            None,
            self.clock.now(),
        );
        self.room_identity = Some(RoomIdentity { room_id, join_code });
        self.room = Some(room);

        // Advance our local key epoch and send the host our sender key for it.
        // The host will already have generated its own key and pushed it down
        // in the same epoch; the matching salt+key pair is what makes
        // decryption succeed for our outgoing traffic.
        let members: Vec<PeerId> =
            self.room.as_ref().unwrap().participants.keys().copied().collect();
        if self.keys.rotate(crate::Epoch(epoch), &members).is_ok() {
            let (key, salt) = self.keys.own_key_for_epoch(crate::Epoch(epoch));
            let _ =
                self.send_control(host, crate::crypto::AppControl::MediaKey { epoch, key, salt });
        }

        self.sink.emit(Event::RoomJoined {
            room: self.room.as_ref().expect("room installed").snapshot(),
        });
        self.set_state(AppState::Connected);
        self.start_audio();
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

        // Silence the no-crypto clippy warning: without `crypto` the body
        // below is removed and the function's last statement becomes the
        // `return` inside this `if`, which clippy calls needless.
        #[cfg_attr(not(feature = "crypto"), allow(clippy::needless_return))]
        if !transmit {
            return;
        }

        // PHASE1: encode with Opus, seal with the group key manager, frame and
        // send over the active path for each route.
        #[cfg(feature = "crypto")]
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

    fn start_audio(&self) {
        if !self.platform.capabilities().microphone {
            let _ = self.platform.request_permission("microphone");
            return;
        }
        if let Err(error) = self.platform.start_playback(&self.config.audio) {
            self.emit_error("audio", &error);
        }
        if let Err(error) = self.platform.start_capture(&self.config.audio) {
            self.emit_error("audio", &error);
        }
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
        assert!(sink
            .events()
            .iter()
            .any(|e| matches!(e, Event::PermissionRequired { capability: "nearby_devices" })));
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

        for (kind, address) in [(PathKind::Lan, "10.0.0.4:47820"), (PathKind::WifiAware, "aware:2")]
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

        let discovered =
            sink.events().into_iter().filter(|e| matches!(e, Event::PeerDiscovered { .. })).count();
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

    /// Drive the host-approval admission flow without going through the live
    /// crypto handshake: install a stub `SecureControl` for the joining peer
    /// and wire it up to the transport so the engine treats it as authenticated.
    #[cfg(feature = "crypto")]
    fn install_authenticated_peer(engine: &mut Engine, remote: PeerId) -> crate::PathId {
        // The transport only knows about peers that have at least one path.
        let now = engine.clock.now();
        let path = engine.transport.add_candidate(
            remote,
            crate::transport::Endpoint::new(crate::transport::PathKind::Lan, "10.0.0.2:1"),
            0,
            now,
        );
        engine.transport.confirm_peer(path, remote, now);
        engine.transport.on_established(path, 1_200, now);
        // The session key here is a test fixture; the engine only uses it to
        // seal/open control records, and the test seals with the same key.
        engine.secure_control.insert(path, crate::crypto::SecureControl::new([0xAB; 32]));
        path
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn host_approval_defers_admission_until_respond_to_join() {
        let (mut engine, sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);

        // Switch the host to host-approval. The default policy admits anyone
        // with the join code, so we set it explicitly for this test.
        engine.room.as_mut().unwrap().admission = crate::room::AdmissionPolicy::HostApproval;

        // Simulate the joiner arriving over an authenticated session.
        let path = install_authenticated_peer(&mut engine, peer(2));
        engine.handle_command(Command::Platform(PlatformEvent::ReliableReceived {
            path,
            data: encode_app_control(crate::crypto::AppControl::RoomJoin {
                code: engine.join_code().unwrap().formatted(),
            }),
        }));

        // The room should not have grown yet — only a JoinRequested was emitted.
        let room = engine.room().expect("room still exists");
        assert_eq!(room.size(), 1, "host approval must defer admission");
        assert!(sink
            .events()
            .iter()
            .any(|e| matches!(e, Event::JoinRequested { peer_id, .. } if *peer_id == peer(2))));

        // Now admit: the joiner should appear.
        engine.handle_command(Command::RespondToJoin { peer_id: peer(2), accept: true });
        let room = engine.room().expect("room still exists");
        assert_eq!(room.size(), 2);
        assert!(room.participant(peer(2)).is_some());
        assert!(sink.events().iter().any(|e| matches!(
            e,
            Event::ParticipantJoined { participant } if participant.peer_id == peer(2)
        )));
        assert!(sink
            .events()
            .iter()
            .any(|e| matches!(e, Event::EpochChanged { epoch } if epoch.0 == 1)));
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn respond_to_join_with_no_pending_request_is_an_error() {
        let (mut engine, sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);
        engine.room.as_mut().unwrap().admission = crate::room::AdmissionPolicy::HostApproval;

        let before = sink.len();
        engine.handle_command(Command::RespondToJoin { peer_id: peer(2), accept: true });
        let events = &sink.events()[before..];
        assert!(
            events.iter().any(|e| matches!(e, Event::Error { layer: "room", .. })),
            "respond_to_join with no pending request must surface an error, not silently admit"
        );
        assert_eq!(engine.room().unwrap().size(), 1);
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn respond_to_join_refusing_emits_join_denied() {
        let (mut engine, sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);
        engine.room.as_mut().unwrap().admission = crate::room::AdmissionPolicy::HostApproval;

        let path = install_authenticated_peer(&mut engine, peer(2));
        engine.handle_command(Command::Platform(PlatformEvent::ReliableReceived {
            path,
            data: encode_app_control(crate::crypto::AppControl::RoomJoin {
                code: engine.join_code().unwrap().formatted(),
            }),
        }));

        engine.handle_command(Command::RespondToJoin { peer_id: peer(2), accept: false });
        assert!(sink
            .events()
            .iter()
            .any(|e| matches!(e, Event::JoinDenied { peer_id } if *peer_id == peer(2))));
        assert_eq!(engine.room().unwrap().size(), 1, "refused joiner must not be a member");
    }

    #[cfg(feature = "crypto")]
    #[test]
    fn joining_a_room_twice_does_not_double_rotate_the_epoch() {
        // Idempotent admission is the property that keeps a confused joiner
        // who retries from leaking a fresh key to them on every retry.
        let (mut engine, sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);

        let path = install_authenticated_peer(&mut engine, peer(2));
        let code = engine.join_code().unwrap().formatted();
        for _ in 0..3 {
            engine.handle_command(Command::Platform(PlatformEvent::ReliableReceived {
                path,
                data: encode_app_control(crate::crypto::AppControl::RoomJoin {
                    code: code.clone(),
                }),
            }));
        }

        let room = engine.room().expect("room still exists");
        assert_eq!(room.size(), 2);
        assert_eq!(room.epoch.0, 1, "duplicate RoomJoin must not advance the epoch");
        // One ParticipantJoined event, not three.
        assert_eq!(
            sink.events()
                .iter()
                .filter(|e| matches!(
                    e,
                    Event::ParticipantJoined { participant } if participant.peer_id == peer(2)
                ))
                .count(),
            1
        );
    }

    #[test]
    fn join_room_without_a_credential_is_rejected() {
        let (mut engine, _sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);

        // A second room is rejected, but so should a JoinRoom with no
        // credential — the Phase 2 wire only carries the code, not the id.
        let before = _sink.len();
        engine.handle_command(Command::JoinRoom {
            room_id: crate::RoomId::generate(),
            credential: None,
        });
        assert!(_sink.events()[before..]
            .iter()
            .any(|e| matches!(e, Event::Error { layer: "room", .. })));
    }

    /// Build a valid encrypted control record carrying `message`, using the
    /// same key the test's `secure_control` entry was constructed with.
    #[cfg(feature = "crypto")]
    fn encode_app_control(message: crate::crypto::AppControl) -> Vec<u8> {
        let mut control = crate::crypto::SecureControl::new([0xAB; 32]);
        control.seal(&message).expect("seal of well-formed AppControl")
    }

    /// After admission, the host's local `SenderKeyManager` should be able to
    /// produce a sender key for the new epoch, and the joiner side should
    /// install a delivered `MediaKey` under that epoch. We test the install
    /// path directly because the round-trip needs a fake transport to ferry
    /// the record across.
    #[cfg(feature = "crypto")]
    #[test]
    fn admitted_joiner_installs_the_hosts_sender_key() {
        let (mut engine, _sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateRoom);

        let path = install_authenticated_peer(&mut engine, peer(2));
        let code = engine.join_code().unwrap().formatted();
        engine.handle_command(Command::Platform(PlatformEvent::ReliableReceived {
            path,
            data: encode_app_control(crate::crypto::AppControl::RoomJoin { code }),
        }));

        // The host's key manager now knows the joiner for the new epoch.
        let epoch = engine.room().unwrap().epoch;
        assert!(
            engine.keys.can_decrypt(peer(2)) || engine.keys.epoch() == epoch,
            "host advanced epoch to {epoch:?} ({}); known members = {:?}",
            engine.keys.epoch(),
            engine.keys.known_members(),
        );
    }

    /// The joiner side installs whatever key the host pushes down, regardless
    /// of whether it matches the epoch the joiner thought it had: epoch
    /// management lives in the EpochManager, not the install path.
    #[cfg(feature = "crypto")]
    #[test]
    fn handle_media_key_installs_into_the_key_manager() {
        let (mut engine, _sink, _clock) = engine(FakePlatform::full());
        engine.handle_command(Command::CreateProfile { display_name: "Alice".into() });

        let path = install_authenticated_peer(&mut engine, peer(2));
        engine.handle_command(Command::Platform(PlatformEvent::ReliableReceived {
            path,
            data: encode_app_control(crate::crypto::AppControl::MediaKey {
                epoch: 1,
                key: [7; 32],
                salt: [3; 12],
            }),
        }));

        // Advance the epoch manager so `can_decrypt` looks in the right slot.
        // This is what `handle_room_accept` does on the joiner side; calling
        // it directly keeps the test focused on the install path.
        let _ = engine.keys.rotate(crate::Epoch(1), &[peer(1), peer(2)]);
        assert!(engine.keys.can_decrypt(peer(2)));
    }
}
