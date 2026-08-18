//! Observability (§92, §93).
//!
//! Available from the first prototype, not bolted on later. Every performance
//! target in §93 — mouth-to-ear latency, join time, relay recovery time, path
//! switch time — is a number that has to be *measured on real devices*, and
//! measuring it requires the instrumentation to already exist.
//!
//! Logged: ids in short form, path metrics, counters, epoch numbers, state
//! transitions.
//!
//! Never logged: private keys, media keys, session secrets, plaintext audio.

mod metrics;

pub use metrics::{Counters, DiagnosticsSnapshot, PathDiagnostics};

/// How often to emit [`crate::Event::Diagnostics`] when diagnostics are on.
///
/// One second: fast enough to watch a path switch happen, slow enough not to
/// become its own source of battery drain.
pub const SNAPSHOT_INTERVAL: core::time::Duration = core::time::Duration::from_secs(1);
