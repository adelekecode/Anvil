//! C ABI bridge between [`anvil_core`] and a host application.
//!
//! Flutter reaches this through `dart:ffi`; the Kotlin and Swift adapters push
//! platform events in through the same surface. Anything that can link C can
//! embed Anvil.
//!
//! ## Design
//!
//! The boundary is deliberately narrow and boring:
//!
//! ```text
//!   anvil_init(config_json)      ─► handle
//!   anvil_command(handle, json)  ─► 0 | error code
//!   anvil_next_event(handle, ms) ─► event JSON | null on timeout
//!   anvil_free_string(ptr)
//!   anvil_shutdown(handle)
//! ```
//!
//! Three choices worth defending:
//!
//! **JSON, not a generated binary ABI.** Control traffic across this boundary is
//! a handful of messages per second — commands and lifecycle events, never
//! media. Media never crosses here at all: audio goes native-adapter → Rust →
//! network without touching Dart. So the cost of JSON is irrelevant, and what
//! it buys is a boundary a human can read in a log and reproduce by hand, with
//! no codegen step in the build.
//!
//! **Blocking event pull, not a callback.** Calling back into Dart from an
//! arbitrary Rust thread requires the Dart VM's port machinery and gets
//! delicate around isolate lifetime. A blocking `anvil_next_event` on a
//! dedicated Dart isolate is a few lines on each side and has no ordering
//! surprises. The engine never blocks on it — events are dropped if the host
//! falls behind, because a stalled UI must not stall the protocol.
//!
//! **Opaque handle, single owner.** The host holds a pointer it cannot look
//! inside. All state lives in the engine thread; nothing is shared.
//!
//! ## Safety
//!
//! Every `extern "C"` function here is `unsafe` by nature. Each one validates
//! its pointers, catches panics at the boundary (a panic unwinding into Dart or
//! Kotlin is undefined behaviour), and returns an error code rather than
//! aborting.

#![allow(clippy::missing_safety_doc)]

use std::collections::VecDeque;
use std::ffi::{c_char, c_int, CStr, CString};
use std::sync::{Arc, Condvar, Mutex};

use anvil_core::{
    AnvilConfig, Command, Engine, EngineHandle, Event, EventSink, PeerId, SystemClock,
};

#[cfg(target_os = "android")]
mod android_platform;
#[cfg(any(target_os = "ios", target_os = "macos"))]
mod apple_platform;
mod convert;
mod platform_bridge;
#[cfg(feature = "quic")]
mod quic_transport;

use convert::{command_from_json, event_to_json, platform_event_from_json};
use platform_bridge::PlatformBridge;

/// Success.
pub const ANVIL_OK: c_int = 0;
/// A pointer argument was null or a string was not valid UTF-8.
pub const ANVIL_ERR_INVALID_ARG: c_int = -1;
/// The command JSON did not parse or named an unknown command.
pub const ANVIL_ERR_BAD_COMMAND: c_int = -2;
/// The engine has stopped.
pub const ANVIL_ERR_ENGINE_STOPPED: c_int = -3;
/// A panic was caught at the boundary.
pub const ANVIL_ERR_PANIC: c_int = -4;

/// Bounded queue of events waiting for the host.
///
/// Bounded because an unbounded one turns a paused UI into unbounded memory
/// growth in the engine. Oldest events are dropped first: during a burst the
/// newest state is what the UI needs.
const EVENT_QUEUE_CAPACITY: usize = 512;

/// Bounded event queue shared between the engine thread and the host.
///
/// When the host falls behind, the **oldest** event is dropped. That direction
/// matters: during a burst — a room filling up, a path flapping — the newest
/// events describe the current state, and it is the stale ones that have no
/// value left.
#[derive(Debug, Default)]
struct EventQueue {
    events: Mutex<VecDeque<Event>>,
    ready: Condvar,
    /// How many events were dropped for overflow, surfaced in diagnostics.
    dropped: Mutex<u64>,
}

impl EventQueue {
    fn push(&self, event: Event) {
        let Ok(mut events) = self.events.lock() else {
            return;
        };

        if events.len() >= EVENT_QUEUE_CAPACITY {
            events.pop_front();
            if let Ok(mut dropped) = self.dropped.lock() {
                *dropped += 1;
            }
        }
        events.push_back(event);
        self.ready.notify_one();
    }

    fn pop_timeout(&self, timeout: std::time::Duration) -> Option<Event> {
        let events = self.events.lock().ok()?;
        let (mut events, _) =
            self.ready.wait_timeout_while(events, timeout, |queue| queue.is_empty()).ok()?;
        events.pop_front()
    }
}

#[derive(Debug)]
struct QueueSink {
    queue: Arc<EventQueue>,
}

impl EventSink for QueueSink {
    fn emit(&self, event: Event) {
        // Never blocks: a stalled host must not stall the protocol.
        self.queue.push(event);
    }
}

/// What the host holds a pointer to.
#[derive(Debug)]
pub struct AnvilSession {
    handle: EngineHandle,
    queue: Arc<EventQueue>,
    platform: Arc<PlatformBridge>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl AnvilSession {
    fn new(config: AnvilConfig, local_peer_id: PeerId) -> Self {
        let queue = Arc::new(EventQueue::default());
        let sink = Arc::new(QueueSink { queue: queue.clone() });
        let platform = Arc::new(PlatformBridge::default());
        let (engine, handle) = Engine::new(
            config,
            platform.clone(),
            sink,
            Arc::new(SystemClock::new()),
            local_peer_id,
        );
        platform.set_engine_handle(handle.clone());

        let thread = std::thread::Builder::new()
            .name("anvil-engine".into())
            .spawn(move || engine.run())
            .expect("failed to spawn engine thread");

        Self { handle, queue, platform, thread: Mutex::new(Some(thread)) }
    }
}

/// Start an Anvil node.
///
/// `config_json` may be null for defaults. Returns an opaque handle, or null on
/// failure. The caller owns the handle and must pass it to [`anvil_shutdown`].
///
/// # Safety
/// `config_json`, if non-null, must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn anvil_init(config_json: *const c_char) -> *mut AnvilSession {
    let result = std::panic::catch_unwind(|| {
        let config = if config_json.is_null() {
            AnvilConfig::default()
        } else {
            let Ok(text) = unsafe { CStr::from_ptr(config_json) }.to_str() else {
                return std::ptr::null_mut();
            };
            convert::config_from_json(text).unwrap_or_default()
        };

        // PHASE2: derived from the stored Ed25519 identity key. Until then a
        // placeholder, so the plumbing is exercisable without key material.
        let local_peer_id = PeerId::UNSPECIFIED;

        Box::into_raw(Box::new(AnvilSession::new(config, local_peer_id)))
    });

    result.unwrap_or(std::ptr::null_mut())
}

/// Submit a command as JSON.
///
/// # Safety
/// `session` must come from [`anvil_init`] and not yet have been shut down.
/// `command_json` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn anvil_command(
    session: *const AnvilSession,
    command_json: *const c_char,
) -> c_int {
    if session.is_null() || command_json.is_null() {
        return ANVIL_ERR_INVALID_ARG;
    }

    let result = std::panic::catch_unwind(|| {
        let session = unsafe { &*session };
        let Ok(text) = unsafe { CStr::from_ptr(command_json) }.to_str() else {
            return ANVIL_ERR_INVALID_ARG;
        };

        let Some(command) = command_from_json(text) else {
            return ANVIL_ERR_BAD_COMMAND;
        };

        if session.handle.send(command) {
            ANVIL_OK
        } else {
            ANVIL_ERR_ENGINE_STOPPED
        }
    });

    result.unwrap_or(ANVIL_ERR_PANIC)
}

/// Submit an event produced by a Kotlin or Swift platform adapter.
///
/// This enters the exact same engine inbox as host commands, which establishes
/// one total ordering between UI actions, network callbacks and audio events.
///
/// # Safety
/// `session` must come from [`anvil_init`] and `event_json` must be a valid
/// NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn anvil_submit_platform_event(
    session: *const AnvilSession,
    event_json: *const c_char,
) -> c_int {
    if session.is_null() || event_json.is_null() {
        return ANVIL_ERR_INVALID_ARG;
    }

    let result = std::panic::catch_unwind(|| {
        let session = unsafe { &*session };
        let Ok(text) = unsafe { CStr::from_ptr(event_json) }.to_str() else {
            return ANVIL_ERR_INVALID_ARG;
        };
        let Some(event) = platform_event_from_json(text) else {
            return ANVIL_ERR_BAD_COMMAND;
        };

        if session.handle.platform(event) {
            ANVIL_OK
        } else {
            ANVIL_ERR_ENGINE_STOPPED
        }
    });

    result.unwrap_or(ANVIL_ERR_PANIC)
}

/// JNI entry point used by `dev.anvil.AnvilPlatform.nativeSubmitEvent`.
#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "system" fn Java_dev_anvil_AnvilPlatform_nativeSubmitEvent(
    mut env: jni::JNIEnv,
    _this: jni::objects::JObject,
    session: jni::sys::jlong,
    event_json: jni::objects::JString,
) -> jni::sys::jint {
    let Ok(text) = env.get_string(&event_json) else {
        return ANVIL_ERR_INVALID_ARG;
    };
    let Ok(text) = text.to_str() else {
        return ANVIL_ERR_INVALID_ARG;
    };
    let Ok(text) = CString::new(text) else {
        return ANVIL_ERR_INVALID_ARG;
    };

    unsafe { anvil_submit_platform_event(session as *const AnvilSession, text.as_ptr()) }
}

/// Attach the Kotlin platform object to the already-running Rust session.
#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "system" fn Java_dev_anvil_AnvilPlatform_nativeAttach(
    env: jni::JNIEnv,
    this: jni::objects::JObject,
    session: jni::sys::jlong,
) -> jni::sys::jint {
    if session == 0 {
        return ANVIL_ERR_INVALID_ARG;
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let vm = env.get_java_vm().map_err(|_| ANVIL_ERR_INVALID_ARG)?;
        let object = env.new_global_ref(this).map_err(|_| ANVIL_ERR_INVALID_ARG)?;
        let session = unsafe { &*(session as *const AnvilSession) };
        session.platform.attach(Arc::new(android_platform::AndroidPlatform::new(vm, object)));
        let _ = session
            .handle
            .platform(anvil_core::PlatformEvent::LifecycleChanged { foreground: true });
        Ok::<_, c_int>(ANVIL_OK)
    }));
    result.unwrap_or(Err(ANVIL_ERR_PANIC)).unwrap_or_else(|code| code)
}

/// Detach the Kotlin object before Flutter destroys its engine.
#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "system" fn Java_dev_anvil_AnvilPlatform_nativeDetach(
    _env: jni::JNIEnv,
    _this: jni::objects::JObject,
    session: jni::sys::jlong,
) {
    if session == 0 {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        let session = unsafe { &*(session as *const AnvilSession) };
        session.platform.detach();
    });
}

/// Attach Swift callbacks to an already-running session.
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[no_mangle]
pub unsafe extern "C" fn anvil_attach_platform(
    session: *mut AnvilSession,
    callbacks: *const apple_platform::AnvilPlatformCallbacks,
) -> c_int {
    if session.is_null() || callbacks.is_null() {
        return ANVIL_ERR_INVALID_ARG;
    }
    let result = std::panic::catch_unwind(|| {
        let session = unsafe { &*session };
        let callbacks = unsafe { *callbacks };
        if callbacks.context.is_null()
            || callbacks.capabilities.is_none()
            || callbacks.invoke.is_none()
            || callbacks.load_identity.is_none()
            || callbacks.release.is_none()
        {
            return ANVIL_ERR_INVALID_ARG;
        }
        session.platform.attach(Arc::new(apple_platform::ApplePlatform::new(callbacks)));
        let _ = session
            .handle
            .platform(anvil_core::PlatformEvent::LifecycleChanged { foreground: true });
        ANVIL_OK
    });
    result.unwrap_or(ANVIL_ERR_PANIC)
}

/// Detach Swift callbacks before the Flutter engine releases its platform host.
#[cfg(any(target_os = "ios", target_os = "macos"))]
#[no_mangle]
pub unsafe extern "C" fn anvil_detach_platform(session: *mut AnvilSession) {
    if session.is_null() {
        return;
    }
    let _ = std::panic::catch_unwind(|| {
        let session = unsafe { &*session };
        session.platform.detach();
    });
}

/// Wait for the next event, up to `timeout_ms`.
///
/// Returns newly allocated JSON the caller must release with
/// [`anvil_free_string`], or null on timeout or shutdown.
///
/// # Safety
/// `session` must come from [`anvil_init`].
#[no_mangle]
pub unsafe extern "C" fn anvil_next_event(
    session: *const AnvilSession,
    timeout_ms: c_int,
) -> *mut c_char {
    if session.is_null() {
        return std::ptr::null_mut();
    }

    let result = std::panic::catch_unwind(|| {
        let session = unsafe { &*session };
        let timeout = std::time::Duration::from_millis(timeout_ms.max(0) as u64);

        let Some(event) = session.queue.pop_timeout(timeout) else {
            return std::ptr::null_mut();
        };

        match CString::new(event_to_json(&event)) {
            Ok(json) => json.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    });

    result.unwrap_or(std::ptr::null_mut())
}

/// Release a string returned by this library.
///
/// # Safety
/// `ptr` must have come from this library and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn anvil_free_string(ptr: *mut c_char) {
    if ptr.is_null() {
        return;
    }
    drop(unsafe { CString::from_raw(ptr) });
}

/// Stop the node and release the handle.
///
/// Blocks until the engine thread has exited, so that the host can be sure no
/// callback or adapter call is still in flight when this returns.
///
/// # Safety
/// `session` must come from [`anvil_init`] and must not be used afterwards.
#[no_mangle]
pub unsafe extern "C" fn anvil_shutdown(session: *mut AnvilSession) {
    if session.is_null() {
        return;
    }

    let _ = std::panic::catch_unwind(|| {
        let session = unsafe { Box::from_raw(session) };
        session.handle.send(Command::Shutdown);

        // Take the join handle out before the session is dropped, or the guard
        // would borrow from a value that is about to go away.
        let thread = session.thread.lock().ok().and_then(|mut slot| slot.take());
        drop(session);

        if let Some(thread) = thread {
            let _ = thread.join();
        }
    });
}

/// Wire protocol version this build speaks, for the host to display.
#[no_mangle]
pub extern "C" fn anvil_protocol_version() -> c_int {
    c_int::from(anvil_core::PROTOCOL_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(text: &str) -> CString {
        CString::new(text).unwrap()
    }

    #[test]
    fn init_and_shutdown_round_trip() {
        let session = unsafe { anvil_init(std::ptr::null()) };
        assert!(!session.is_null());
        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn commands_are_accepted() {
        let session = unsafe { anvil_init(std::ptr::null()) };

        let command = c(r#"{"type":"createRoom"}"#);
        assert_eq!(unsafe { anvil_command(session, command.as_ptr()) }, ANVIL_OK);

        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn platform_events_are_accepted() {
        let session = unsafe { anvil_init(std::ptr::null()) };
        let event = c(r#"{"type":"networkChanged","kind":"lan","available":true}"#);

        assert_eq!(unsafe { anvil_submit_platform_event(session, event.as_ptr()) }, ANVIL_OK);

        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn malformed_platform_events_are_rejected() {
        let session = unsafe { anvil_init(std::ptr::null()) };
        let event = c(r#"{"type":"networkChanged","kind":"bluetooth","available":true}"#);

        assert_eq!(
            unsafe { anvil_submit_platform_event(session, event.as_ptr()) },
            ANVIL_ERR_BAD_COMMAND
        );

        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn null_arguments_are_rejected_rather_than_crashing() {
        let command = c(r#"{"type":"mute"}"#);
        assert_eq!(
            unsafe { anvil_command(std::ptr::null(), command.as_ptr()) },
            ANVIL_ERR_INVALID_ARG
        );

        let session = unsafe { anvil_init(std::ptr::null()) };
        assert_eq!(unsafe { anvil_command(session, std::ptr::null()) }, ANVIL_ERR_INVALID_ARG);
        assert!(unsafe { anvil_next_event(std::ptr::null(), 1) }.is_null());
        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn malformed_commands_are_rejected() {
        let session = unsafe { anvil_init(std::ptr::null()) };

        for text in [r#"{"#, r#"{"type":"nonsense"}"#, r#"[]"#, r#""#] {
            let command = c(text);
            assert_eq!(
                unsafe { anvil_command(session, command.as_ptr()) },
                ANVIL_ERR_BAD_COMMAND,
                "accepted {text:?}"
            );
        }

        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn events_flow_to_the_host() {
        let session = unsafe { anvil_init(std::ptr::null()) };
        let command = c(r#"{"type":"createRoom"}"#);
        unsafe { anvil_command(session, command.as_ptr()) };

        let mut saw_room = false;
        for _ in 0..40 {
            let ptr = unsafe { anvil_next_event(session, 100) };
            if ptr.is_null() {
                continue;
            }
            let json = unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned();
            unsafe { anvil_free_string(ptr) };
            if json.contains("roomCreated") {
                saw_room = true;
                break;
            }
        }

        assert!(saw_room, "createRoom produced no roomCreated event");
        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn polling_an_idle_session_times_out_without_blocking_forever() {
        let session = unsafe { anvil_init(std::ptr::null()) };

        // Drain whatever startup produced.
        while !unsafe { anvil_next_event(session, 50) }.is_null() {}

        let started = std::time::Instant::now();
        assert!(unsafe { anvil_next_event(session, 50) }.is_null());
        assert!(started.elapsed() < std::time::Duration::from_secs(2));

        unsafe { anvil_shutdown(session) };
    }

    #[test]
    fn freeing_a_null_string_is_safe() {
        unsafe { anvil_free_string(std::ptr::null_mut()) };
    }

    #[test]
    fn protocol_version_is_reported() {
        assert_eq!(anvil_protocol_version(), c_int::from(anvil_core::PROTOCOL_VERSION));
    }
}
