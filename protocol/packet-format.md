# Packet format

## 1. Media packet

```
 ┌────────────────────────────────────┐
 │ MediaHeader              23 bytes  │  relay-visible, authenticated as AAD
 ├────────────────────────────────────┤
 │ encrypted Opus frame               │  sealed to room members
 │ AEAD tag                 16 bytes  │
 └────────────────────────────────────┘
```

### 1.1 Header layout

Big-endian throughout.

```
 offset  size  field              relay needs it for
 0       1     version            rejecting incompatible traffic early
 1       1     packet_type        deciding whether it is forwardable
 2       1     flags              hop marking, loop prevention
 3       4     room_route_id      which room's members to fan out to
 7       4     sender_route_id    who not to echo the packet back to
 11      2     stream_id          per-stream ordering at the receiver
 13      4     sequence           loss detection, replay rejection
 17      4     timestamp          jitter buffer playout spacing
 21      2     epoch              which key generation to use
```

Route ids are the leading 4 bytes of the `PeerId` / `RoomId`. Truncation means
collisions are possible but vanishingly unlikely among a handful of members; a
collision is resolved by **refusing to route**, not by guessing, because
attributing a packet to the wrong sender would corrupt replay state for both.

### 1.2 Flags

| Bit | Name | Meaning |
|---|---|---|
| 0 | `RELAYED` | already forwarded once |
| 1 | `TALKSPURT_START` | first packet after VAD silence |

`RELAYED` is the forwarding-loop guard. During a relay change, two nodes can
briefly each believe they are the relay; without this they forward to each other
at line rate until a battery dies.

### 1.3 The overhead question

23 + 16 = **39 bytes of overhead on a ~60-byte frame** — about 15 kbps per
stream at 50 packets/second, on top of a 24 kbps payload. That is a real cost,
and on a congested radio with four participants it is the difference between
fitting and not.

Accepted for v1 because the alternative — implicit or compressed header state —
requires per-path context the relay would have to maintain, and getting that
wrong breaks recovery after a relay change. A future version can negotiate
compression, which is exactly why the version byte is first.

### 1.4 What is deliberately absent

No full peer id, no room id, no display name, no participant list. A relay learns
that a packet exists, roughly how big it is, and when. It does not learn who is
in the room or what was said.

### 1.5 Associated data

The header is the AEAD associated data, **with the `RELAYED` bit masked to
zero**.

This is what binds the visible routing fields to the sealed payload: a relay can
*read* the sequence number and epoch but cannot alter either undetected, which is
what stops a malicious relay from re-sequencing a stream to force a replay or
scramble playout order. Masking the relay bit lets the relay set it without
holding a key.

## 2. Packet types

| Value | Name | Channel | Relayable |
|---|---|---|---|
| 0x01 | `HELLO` | reliable | no |
| 0x02 | `IDENTITY` | reliable | no |
| 0x10 | `ROOM_CREATE` | reliable | no |
| 0x11 | `ROOM_JOIN` | reliable | no |
| 0x12 | `ROOM_ACCEPT` | reliable | no |
| 0x13 | `ROOM_LEAVE` | reliable | no |
| 0x14 | `MEMBERSHIP` | reliable | no |
| 0x20 | `KEY_EXCHANGE` | reliable | no |
| 0x21 | `KEY_ROTATE` | reliable | no |
| 0x30 | `RELAY_ANNOUNCE` | reliable | no |
| 0x31 | `RELAY_ELECTION` | reliable | no |
| 0x32 | `RELAY_SWITCH` | reliable | no |
| 0x40 | `PATH_PROBE` | datagram | no |
| 0x41 | `PATH_REPORT` | datagram | no |
| 0x42 | `HEARTBEAT` | datagram | **yes** |
| 0x50 | `MEDIA` | datagram | **yes** |
| 0xF0 | `ERROR` | reliable | no |

Discriminants are fixed for the life of protocol v1.

**Exactly two types are relayable, and that is enforced by test.** A relay that
could forward a `MEMBERSHIP` or `KEY_EXCHANGE` would have a hand in who is in the
room and who holds keys — the precise privilege escalation the architecture
exists to prevent.

## 3. Parsing rules

Any device in radio range can send bytes at these parsers with no prior
relationship. Therefore:

1. **Check lengths before indexing.** Every decode path.
2. **Reject unknown values.** Unknown version, unknown packet type, oversized
   length prefix — error, never a guess.
3. **Never panic.** Asserted by tests that feed every truncation and a spread of
   arbitrary bytes through each parser.
4. **Reject a packet too small to hold a tag** before it reaches code that holds
   keys.

## 4. Control messages — *unpinned*

The message set is fixed; the binary encoding is not.

| Message | Payload |
|---|---|
| `Hello` | version, fingerprint |
| `Identity` | Ed25519 public key, X25519 ephemeral, signature, display name |
| `RoomJoin` | room id, optional credential |
| `RoomAccept` | room id, epoch, members, relay |
| `RoomLeave` | peer, reason |
| `Membership` | epoch, added[], removed[] |
| `KeyExchange` | epoch, sealed key (per-recipient) |
| `KeyRotate` | epoch |
| `RelayAnnounce` | score, term |
| `RelayElection` | term, candidate |
| `RelaySwitch` | term, relay |
| `Error` | code, detail |

The encoding should be settled alongside the Phase 1 QUIC work, when the exact
field list is known from working code. Fixing an encoding first and discovering
the field list second is how wire formats acquire reserved bytes that never get
used.

Note `KeyExchange` is **per-recipient**, sent over that recipient's authenticated
session. It is not fan-out traffic and must never pass through a relay.
