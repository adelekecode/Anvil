# Anvil

What if a group voice room did not begin with a login screen?

What if four phones could find one another, form a room, and keep talking after
the router lost its internet connection? What if the room had no server to
trust, no account database to query, and no central relay that could read the
conversation?

Anvil is an experiment in that direction: an offline-first, end-to-end
encrypted communication system for nearby devices. It combines a Flutter
mobile interface with a Rust engine that owns identity, rooms, transport,
encryption, routing, chat, and voice media. Android and iOS provide the radio,
audio, lifecycle, and secure-storage capabilities around it.

The interesting constraint is also the product idea: Anvil should remain useful
when the usual infrastructure disappears.

## The short version

Anvil is designed for:

- small voice rooms between nearby phones;
- no accounts, servers, phone numbers, or internet requirement;
- local discovery over Wi-Fi, with Android Wi-Fi Aware for routerless paths;
- end-to-end encrypted media, including when traffic crosses a relay;
- transport failover when a path becomes slow, unreachable, or disappears;
- a single platform-independent protocol core, so Android and iOS do not make
  different decisions about rooms or security.

The current codebase is beyond a UI mockup. The Rust engine, C ABI, Flutter
client, Android/iOS bridges, QUIC LAN transport, Opus voice path, encryption
bookkeeping, and Android Wi-Fi Aware adapter are implemented and covered by
automated tests. The remaining uncertainty is primarily empirical: real-device
radio behavior, cross-platform discovery, background execution, and audio
tuning still need testing on physical phones.

## Follow the conversation

Anvil can be understood as a chain of questions.

### How do nearby devices find each other?

Each device publishes a small advertisement containing a short identity
fingerprint and, when relevant, a room hint. The advertisement is deliberately
not proof of identity. It is only an invitation to begin a conversation.

On a local Wi-Fi network, Anvil uses Bonjour/NSD discovery. On Android devices
with supported hardware and permissions, the Wi-Fi Aware adapter publishes and
subscribes to the same service name without requiring a router. A device may be
seen through both paths; the Rust core folds those sightings back into one peer
while keeping both paths available.

See [`protocol/discovery.md`](protocol/discovery.md) for the details and the
limits of discovery data.

### How can there be identity without accounts?

The device is the account.

On first launch, Anvil creates a local keypair and derives a `PeerId` from its
public key. A display name is just a label. Two people can both be called
“Femi”; their cryptographic identities remain different.

Trust is explicit and local. A peer encountered for the first time is shown as
unverified. If a known peer later presents a different key, Anvil raises a
warning instead of quietly replacing the identity.

```text
first launch  →  create local identity  →  advertise fingerprint
meeting       →  authenticate identity  →  remember trust decision
reopening     →  load local identity    →  discover nearby peers
```

The identity model and its trade-offs live in
[`protocol/identity.md`](protocol/identity.md).

### What is a room, if connections can vanish?

A room is not a socket.

`RoomId`, membership, key epochs, message history, and call state live above
the network paths. A Wi-Fi connection can fail while the room remains intact;
the transport manager can promote another path or wait for reconnection.

For groups larger than two, Anvil can elect one participant as a relay. The
relay forwards already-sealed media packets. It does not become the authority,
and it does not receive the sender keys needed to decrypt everybody’s voice.
If that relay disappears, the remaining members can elect another one without
changing the room’s trust model.

These are not just diagrams. The room, relay, routing, replay, failover, and
media-timing behaviors are exercised with deterministic clocks and fake
transports in the Rust test suite.

### How does voice survive a bad packet?

Voice is treated as a stream of moments, not a queue of obligations.

The audio path is:

```text
microphone
   ↓
native PCM handoff
   ↓
Rust resampling + VAD
   ↓
Opus encode
   ↓
end-to-end media encryption
   ↓
QUIC datagram / Wi-Fi Aware message
   ↓
replay check + decrypt
   ↓
jitter buffer + Opus PLC decode
   ↓
Rust mixer
   ↓
speaker
```

Lost voice frames are allowed to disappear. Late audio is usually worse than
missing audio, so media uses unreliable datagrams while room control uses a
reliable ordered channel. The mixer, jitter buffer, resampler, VAD, and Opus
engine are shared across platforms rather than reimplemented in Dart.

### What happens when the network is not normal?

Anvil assumes that the network will be awkward:

- a router may provide Wi-Fi but no internet;
- Android may prefer cellular unless sockets are scoped correctly;
- guest networks may isolate clients from one another;
- a peer may move between Wi-Fi paths;
- a phone call may interrupt the microphone;
- a relay may run low on battery or disappear;
- packets may be duplicated, reordered, delayed, or forged.

The core responds to observations rather than making the platform adapters
decide policy. Kotlin and Swift report capabilities and events; Rust decides
which path to use, when to fail over, whether a packet is authentic, and how
the room should change.

## Architecture

```text
┌─────────────────────────────────────────────────────────────┐
│ Flutter                                                      │
│ screens, controls, accessible state, event presentation      │
└──────────────────────┬──────────────────────────────────────┘
                       │ JSON commands/events over C ABI
┌──────────────────────▼──────────────────────────────────────┐
│ Rust / anvil-core                                            │
│ identity · rooms · crypto · chat · calls · Opus · routing    │
│ relay election · path scoring · failover · replay protection  │
└──────────────────────┬──────────────────────────────────────┘
                       │ narrow platform capability boundary
┌──────────────────────▼──────────────────────────────────────┐
│ Kotlin / Swift                                               │
│ discovery · Wi-Fi Aware · audio · lifecycle · key storage     │
└──────────────────────┬──────────────────────────────────────┘
                       │
             Wi-Fi LAN · AWDL peer-to-peer · Android Aware
```

The boundary is intentionally asymmetric: the platform performs, while the
core decides. This keeps protocol behavior testable on a laptop and prevents
Android and iOS from slowly growing two incompatible versions of Anvil.

## Where the project stands

The most useful distinction is between “implemented in the engine” and
“proven on real radios.”

| Area | Current state |
| --- | --- |
| Rust protocol engine | Room lifecycle, identities, calls, chat, routing, relay logic, path scoring, failover, replay handling, and media timing are implemented. |
| Cryptography | Ed25519/X25519 identity, authenticated handshakes, sender-key epochs, ChaCha20-Poly1305 media, and replay windows are in the Rust runtime path. |
| Voice | Native PCM enters Rust; Opus encoding/decoding, jitter buffering, PLC, mixing, encryption, and playback are wired through the mobile builds. |
| LAN transport | Mobile builds use the Rust QUIC data plane for LAN paths, with native discovery and Wi-Fi-scoped platform configuration. |
| Android Wi-Fi Aware | Real publish/subscribe discovery and session-local framed transport are implemented. Reliable control records are fragmented, reassembled, acknowledged, and retried. |
| iOS peer-to-peer | iOS has no public Android-style Wi-Fi Aware/NAN API. Its Network.framework LAN path opts into peer-to-peer/AWDL where available; it does not claim a fake Aware capability. |
| Flutter shell | First-run identity, discovery, peer trust, calls, rooms, chat, diagnostics, and lifecycle presentation are connected to the core event stream. |
| Device validation | Builds and deterministic tests pass. Physical-device matrix testing remains the next proof point. |

The iOS Opus build has a small extra piece of toolchain glue because the
upstream bundled Opus build uses Autoconf: the repository’s
[`opus_cc_wrapper.sh`](apps/mobile/ios/opus_cc_wrapper.sh) applies iPhoneOS
flags only inside the temporary Opus build and leaves Cargo’s host tools on the
macOS SDK.

## Repository map

```text
crates/anvil-core/     platform-independent protocol engine
  identity/            local identity, fingerprints, trust-on-first-use
  room/                membership, join codes, epochs, room state
  transport/           paths, metrics, scoring, failover
  relay/               election, health, forwarding
  audio/               PCM, VAD, resampling, jitter, mixer, Opus
  crypto/              identity, handshake, keys, AEAD, replay windows

crates/anvil-ffi/      C ABI, JSON conversion, native platform bridge, QUIC
apps/mobile/lib/       Flutter UI, controller, models, FFI client
apps/mobile/android/   Android adapters, Wi-Fi Aware, Rust/JNI build hook
apps/mobile/ios/       iOS adapters, peer-to-peer LAN, Rust/Xcode build hook
protocol/              design notes and wire-format documents
tests/                 physical-device scenarios and validation plans
```

## Try the checks

From the repository root:

```bash
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

For the Flutter shell:

```bash
cd apps/mobile
flutter pub get
flutter test
flutter analyze
```

Build the mobile packages when the relevant SDKs are installed:

```bash
flutter build apk --debug
flutter build ios --no-codesign
```

Android’s native build uses `cargo-ndk` and currently targets `arm64-v8a`.
iOS’s native build targets a physical `aarch64-apple-ios` device. The iOS
command above intentionally skips signing; deployable builds still need an
Apple team and provisioning profile.

## Read further

Start with [`protocol/specification.md`](protocol/specification.md) for the
design map, then explore the parts that raise the most interesting questions:

1. [`protocol/discovery.md`](protocol/discovery.md) — finding peers without a
   registry.
2. [`protocol/identity.md`](protocol/identity.md) — names, keys, and trust.
3. [`protocol/transport.md`](protocol/transport.md) — choosing and replacing
   paths.
4. [`protocol/encryption.md`](protocol/encryption.md) — transport security,
   end-to-end media, epochs, and replay rejection.
5. [`protocol/relay-election.md`](protocol/relay-election.md) — why a relay is
   useful but never an authority.
6. [`tests/README.md`](tests/README.md) — the real-device experiments still to
   run.

## The experiment

The long-term test is deliberately simple to describe:

> Put three or four phones in the same place. Disconnect the internet. Start a
> room. Talk. Unplug the router, move between paths, let one phone leave, and
> see whether the conversation remains understandable and the trust model
> remains intact.

Anvil is an attempt to make that experiment ordinary.
