# Anvil protocol v1

Status: **draft**. Normative for Phase 0 interfaces; sections marked *unpinned*
must be fixed before Phase 1 code stabilises.

## 1. What this protocol is for

Anvil carries real-time group voice between nearby devices with no internet, no
cellular, no accounts and no servers. It uses whichever local path is best
available — an existing Wi-Fi LAN, or Wi-Fi Aware when there is no router — and
switches between them without interrupting the conversation.

## 1.1 A fourth invariant: no accounts

There is no signup, no login, no server and no authority. A user's identity is a
keypair generated on their device on first launch, from nothing but a display
name they type. See [identity.md](identity.md).

Consequence: nothing in this protocol may require a value that only a server
could issue, and nothing may treat a display name as an identity.

## 2. The three invariants

Everything else in this document follows from three statements. If a proposed
change violates one, the change is wrong.

### 2.1 A room is not a connection

`RoomId`, `PeerId`, `StreamId`, sequence state and key epochs are defined
independently of any socket, IP address or radio. A path can fail and be
replaced without the room, the membership or the cryptographic state changing.

Consequence: no identifier in this protocol may be derived from a network
address, a relay, or a transport.

### 2.2 A relay is not an authority

The elected relay forwards sealed packets. It holds no media keys, decides no
membership, and can be replaced at any time. If the relay device belongs to a
participant, that person can hear the room because they are a *participant* —
the relay role adds nothing.

Consequence: no control message may be relayed, and no key material may be
derivable from anything a relay sees.

### 2.3 The core decides, the platform performs

Transport selection, failover, relay election, peer identity and media timing
are decided once, in the protocol, for both platforms. Adapters expose
capabilities and report events.

Consequence: any behaviour that differs between Android and iOS is either an OS
constraint being reported honestly, or a bug.

## 3. Layering

```
  Application (rooms, participants, controls)
        │
  Anvil protocol  ─ control messages ─┐
        │         ─ media packets ────┼── end-to-end media encryption
        │                             │
  QUIC  ─ reliable streams (control) ─┘
        ─ datagrams (media)              ── transport security, per hop
        │
  UDP / IP
        │
  Wi-Fi LAN  ·  Wi-Fi Aware
```

Two encryption layers, protecting against different adversaries:

| Layer | Protects against | Terminated at |
|---|---|---|
| End-to-end media | the relay, and anyone on any hop | the two endpoints |
| QUIC/TLS transport | attackers on one hop | each hop, including the relay |

Neither substitutes for the other. The relay terminates QUIC; if media were only
protected by transport security, the relay would see plaintext.

## 4. Document map

| Document | Covers |
|---|---|
| [identity.md](identity.md) | no accounts, peer ids, fingerprints, trust on first use, join codes |
| [discovery.md](discovery.md) | finding peers, advertisement format, de-duplication |
| [transport.md](transport.md) | paths, metrics, scoring, failover |
| [packet-format.md](packet-format.md) | media header, packet types, control framing |
| [encryption.md](encryption.md) | identity, handshake, sender keys, epochs, replay |
| [relay-election.md](relay-election.md) | candidacy, scoring, elections, health |
| [room-lifecycle.md](room-lifecycle.md) | creation, admission, membership, departure |
| [failure-recovery.md](failure-recovery.md) | what breaks, what happens, what the user sees |

## 5. Versioning

`PROTOCOL_VERSION = 1`, carried as the first byte of every media packet and in
the `Hello` control message.

Anvil has no update server and no way to tell a user to upgrade. Two phones in a
field run whatever they last had. So: **refuse clearly, never half-support.** A
peer advertising an unknown version is rejected at handshake with a specific
error the UI can explain. It is never accepted and then fed packets it will
misparse.

A future v2 build may keep speaking v1 for a release or two — the supported set
is a list, not a single value.

## 6. Constants

| Constant | Value | Where |
|---|---|---|
| Service name | `_anvil._udp` | LAN and Aware, identically |
| Default LAN port | 47820 | advertised, not assumed |
| Media header | 23 bytes | [packet-format.md](packet-format.md) |
| AEAD tag | 16 bytes | ChaCha20-Poly1305 |
| Frame duration | 20 ms | Opus, 48 kHz mono |
| Replay window | 64 packets | ~1.3 s |
| Epoch retention | 500 ms | [encryption.md](encryption.md) |

## 7. What is deliberately not here

- **Multi-hop routing.** Direct connectivity plus one elected relay (§36). Phones
  make poor routers: the OS backgrounds them and the radio sleeps.
- **MLS.** Per-sender keys for v0.1, behind a `GroupKeyManager` abstraction so the
  replacement is tractable. See [encryption.md](encryption.md) for what this
  costs.
- **Packet duplication across paths.** One active path, one warm standby. Sending
  every frame twice doubles airtime and battery to buy redundancy that failover
  already provides in a few hundred milliseconds.
- **Traffic-analysis resistance.** A relay learns that a participant transmitted,
  roughly how much, and when. Reducing that is a v2 conversation.

## 8. Unpinned before Phase 1

1. **Control message binary encoding.** The message set is fixed
   ([packet-format.md](packet-format.md) §4); the encoding is not. It should be
   settled alongside the QUIC work, when the exact field list is known from
   working code rather than guessed from a diagram.
2. **Handshake transcript definition.** Exactly which bytes the identity
   signature covers. Getting this wrong is a downgrade or replay vulnerability,
   so it needs writing down before it is implemented, not after.
3. **Join credential format.** Whether the join code is compared directly or used
   to derive a proof. See [room-lifecycle.md](room-lifecycle.md) §3.
