# Anvil

Anvil is an offline-first, end-to-end encrypted group voice project for nearby
devices. The target is to work without internet access, cellular service,
accounts, or servers—first over an ordinary local Wi-Fi network, and eventually
over Wi-Fi Aware when there is no router.

> [!IMPORTANT]
> **Current status: architecture prototype, not a working voice app.** The
> Phase 0 foundation is substantially in place: protocol documentation, the
> Rust engine and state machines, a C ABI, the Flutter UX, native app shells,
> and platform-adapter/build scaffolding. Real discovery, connections, audio,
> persistent identity, and encryption are not connected end to end yet. The
> next milestone is the Phase 1 LAN proof of concept.

## What exists today

| Area | Current state |
|---|---|
| Protocol and architecture | Specifications cover identity, discovery, packet formats, encryption, room lifecycle, relay election, transport selection, and failure recovery. |
| Rust core | Deterministic engine loop plus tested room, call, chat, discovery, routing, relay, packet, jitter-buffer, mixer, VAD, replay-window, and path-failover logic. |
| Host boundary | A working C ABI accepts JSON commands and exposes a bounded event queue; Dart bindings consume it on a background isolate. |
| Flutter app | First-run, home, peer, direct-call, chat, room, trust-warning, and diagnostics surfaces are implemented and fold core events through one controller. |
| Native projects | Android and iOS projects include permissions, lifecycle, key-store, LAN, Wi-Fi Aware, and audio adapter seams. Android Gradle and the iOS Xcode project invoke Rust build scripts. |
| Verification | `cargo test --workspace` passes 295 tests; the Flutter suite passes 21 tests; Rust clippy and Flutter analysis are clean. |

The implemented core logic is intentionally device-independent. It can model
peer discovery, path loss, relay failure, room membership, and media timing with
an injected clock and fake platform, which is why those behaviours are testable
before the radio and audio adapters are live.

## What does not work yet

- The FFI session currently starts the Rust engine with `NullPlatform`; the
  Kotlin and Swift adapters are not attached to that engine.
- The platform-event entry points are incomplete, including the Android JNI
  implementation and the Rust symbol called by Swift.
- Android NSD, iOS Bonjour/`NWBrowser`, QUIC connections, and reliable control
  messages are not implemented. Joining a room, admitting a participant,
  calling a peer, and sending a message therefore do not reach another device.
- Opus encode/decode and microphone/speaker I/O are stubs.
- Ed25519 identity generation, signing, authenticated handshakes, sender-key
  derivation, ChaCha20-Poly1305 media protection, and persistent identity loading
  are stubs. The current profile uses a placeholder identity and is not restored
  across launches.
- Relay selection, forwarding, replay rejection, and adaptive path choice are
  implemented as core logic, but are not driven by real network traffic.
- Android Wi-Fi Aware is scaffolded only. Cross-platform Wi-Fi Aware support is
  still an open hardware/API question on iOS.

In particular, the current build must not be treated as secure: the product's
end-to-end encryption design is documented and its bookkeeping is tested, but
the cryptographic operations are not yet in the runtime path.

## No-accounts model

The intended identity model has no signup, login, password, phone number, or
server. On first launch, a user chooses a display name and Anvil generates a
long-lived keypair locally:

```text
Install  ->  choose a display name  ->  generate and store a keypair  ->  ready
Reopen   ->  load local identity    ->  start discovery               ->  ready
```

| Conventional system | Anvil design |
|---|---|
| account | an on-device keypair |
| user ID | `PeerId`, derived from the public key |
| login | loading the local profile |
| session token | none |
| display name | a label, deliberately not an identity |

Two people may both be called “Femi.” Anvil distinguishes them by `PeerId` and
shows a short fingerprint when it matters. Trust is trust-on-first-use: a known
peer presenting a different key raises an explicit warning rather than being
silently accepted. See [`protocol/identity.md`](protocol/identity.md) for the
model and its limits.

The UI and state machine for this flow exist today; secure key generation and
persistence are Phase 2 work.

## Architecture

```text
Flutter          screens, controls, state display
    |  commands in, events out; never protocol state
Rust core        identity, rooms, crypto, media, transport, relay
    |  capabilities in, platform commands out
Kotlin / Swift   Wi-Fi Aware, LAN, microphone, lifecycle, secure storage
    |
Wi-Fi LAN  /  Wi-Fi Aware
```

Three invariants shape the design:

**A room is not a connection.** `RoomId`, `PeerId`, `StreamId`, sequence state,
and key epochs are independent of sockets, IP addresses, and radios. Replacing
a failed path should not replace the room.

**A relay is not an authority.** Group media is designed to pass through an
elected participant that forwards sealed packets without holding other senders'
media keys. Replacing the relay should not change the room's trust model.

**The core decides; the platform performs.** Transport selection, failover,
relay election, identity, and media timing live in Rust. Kotlin and Swift expose
OS capabilities and carry out requests without duplicating protocol policy.

## Repository layout

```text
crates/anvil-core/     protocol engine and platform-independent policy
  identity/            profiles, fingerprints, known peers, TOFU bookkeeping
  peer/                peer relationships and direct-call state machine
  chat/                messages and in-memory history
  room/                membership, join codes, epochs, and room state
  transport/           path metrics, scoring, and failover
  relay/               election, health monitoring, and forwarding rules
  audio/               PCM frames, jitter buffer, VAD, mixer, Opus seam
  crypto/              identity, handshake, replay, epoch, and sender-key seams
crates/anvil-ffi/      C ABI and JSON command/event conversion
apps/mobile/lib/       Flutter application, controller, models, and FFI client
apps/mobile/android/   Kotlin platform adapters and Rust build hook
apps/mobile/ios/       Swift platform adapters and Rust build hook
protocol/              protocol and architecture documents
tests/                 real-device test plans for later milestones
```

## Development

### Prerequisites

- Rust stable, with `rustfmt` and `clippy` (the crates require Rust 1.82 or
  newer)
- Flutter 3.22 or newer with Dart 3.4 or newer for the mobile shell

### Checks that work now

From the repository root:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

For Flutter:

```bash
cd apps/mobile
flutter pub get
flutter test
flutter analyze
```

The Flutter unit tests do not load the Rust dynamic library; they exercise event
decoding and UI-facing model behaviour. Running on a phone additionally enters
the unfinished native integration described above.

### Native build scaffolding

- Android's `preBuild` task calls `apps/mobile/android/build_rust.sh`, which
  expects `cargo-ndk` and currently builds only `arm64-v8a` into `jniLibs`.
- The iOS Xcode project calls `apps/mobile/ios/build_rust.sh`, which currently
  builds `aarch64-apple-ios` for a physical device. Simulator targets and final
  static-library linkage still need to be completed.

The Cargo features below are off by default. Enabling one currently makes its
dependencies available; it does **not** imply the subsystem is complete.

| Feature | Dependency surface | Implementation state |
|---|---|---|
| `crypto` | Ed25519, X25519, HKDF, SHA-256, ChaCha20-Poly1305 | SHA-256 peer-ID derivation exists; key lifecycle, signatures, handshake, and AEAD are pending. |
| `quic` | quinn and rustls | Constants and interface seam only. |
| `opus` | libopus through `audiopus` | Interface seam only; encode/decode return `NotImplemented`. |

## Implementation roadmap

The phases describe operational milestones. Some later-phase algorithms were
implemented early as pure logic, but they are not considered complete until
they run between real devices.

| Phase | Goal | Current state |
|---|---|---|
| 0 | Repository, interfaces, protocol docs, app shell | **Substantially complete**; native bridge attachment/linkage remains. |
| 1 | LAN discovery, QUIC, Opus, and real audio | **Next**; interfaces and build hooks only. |
| 2 | Persistent identity, authenticated handshake, sender keys, and epochs | Bookkeeping and tests exist; cryptographic operations are pending. |
| 3 | Encrypted relay fan-out and relay failover | Election, health, routing, and forwarding logic exist; network coordination is pending. |
| 4 | Android Wi-Fi Aware | Adapter scaffold only. |
| 5 | iOS peer-to-peer path and cross-platform interop | Unvalidated; highest schedule risk. |
| 6 | Simultaneous paths and adaptive failover | Scoring/failover logic exists; live multi-path integration is pending. |
| 7 | Audio measurement and tuning | VAD, jitter, and mixing logic exist; codec/device tuning is pending. |
| 8 | Reliability, security, and stress validation | Device test plans exist; execution depends on the earlier phases. |

The success criterion remains: **three or four phones, no internet, and a secure
group conversation**, both on a router with its WAN disconnected and on a
routerless local path.

## Known risks

**iOS peer-to-peer interop is the largest schedule risk.** Whether the available
Apple APIs can establish the required path with Android Wi-Fi Aware is an
empirical question. It should be tested with a small two-device probe before
the later phases depend on it. The protocol can fall back to a local network
hosted by one device without changing room identity or encryption.

**Sender keys are deliberately sized for small rooms.** The v0.1 design requires
O(n²) key deliveries on membership changes. That is acceptable for three or
four participants, not for larger rooms. `GroupKeyManager` is the replacement
seam for a future MLS-based design.

## Reading order

1. [`protocol/specification.md`](protocol/specification.md) — invariants and the
   full document map.
2. [`crates/anvil-core/src/lib.rs`](crates/anvil-core/src/lib.rs) — the core's
   ownership and platform-boundary model.
3. [`crates/anvil-core/src/engine.rs`](crates/anvil-core/src/engine.rs) — what is
   handled now and where operational stubs remain.
4. [`protocol/transport.md`](protocol/transport.md) — path scoring and failover.
5. [`protocol/encryption.md`](protocol/encryption.md) and
   [`protocol/identity.md`](protocol/identity.md) — the security design and its
   explicit trade-offs.
6. [`tests/README.md`](tests/README.md) — device scenarios that become runnable
   as the platform milestones land.
