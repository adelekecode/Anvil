# Failure and recovery

What breaks, what Anvil does about it, and what the user sees. Written as a table
of cases rather than prose, because the useful question during an incident is
"which row is this".

## 1. The general rule

**Degrade, do not collapse.** Almost nothing in this system should end a room.
The room is the thing the user cares about; paths, relays and packets are
implementation detail that may fail freely as long as the conversation survives.

The recovery hierarchy, cheapest first:

```
 conceal a frame        ~20 ms    nobody notices
 switch path            ~100 ms   brief artefact
 elect a new relay      ~1.5 s    audible gap, room intact
 re-establish session   seconds   room intact, keys intact
 end the room                     only when there is nobody left
```

## 2. Cases

### 2.1 A packet is lost

| | |
|---|---|
| Detected by | sequence gap in the jitter buffer |
| Response | Opus packet-loss concealment synthesises a replacement |
| Room | untouched |
| User sees | nothing at 1–2%; slight roughness above that |

A gap is not a failure. A buffer that stalled waiting for a frame that is never
coming would turn a 20 ms artefact into a broken call.

### 2.2 Packets arrive out of order

| | |
|---|---|
| Detected by | jitter buffer |
| Response | reorder within the buffer depth; the depth adapts upward fast |
| User sees | nothing |

Frames arriving after their playout slot are dropped, not played late.

### 2.3 A path degrades

| | |
|---|---|
| Detected by | path scoring |
| Response | switch, if the alternative wins by ≥15 after a 10 s dwell |
| Room | untouched |
| User sees | nothing |

### 2.4 The active path dies (router unplugged)

| | |
|---|---|
| Detected by | 3 s without traffic, or an explicit `PathLost` from the adapter |
| Response | immediate failover to standby, bypassing dwell and hysteresis |
| Room | untouched — same `RoomId`, `PeerId`, epoch, sequence state |
| User sees | brief gap; `Reconnecting` if no standby is ready |

This is the case the whole transport architecture exists for. The session lives
above the path, so nothing above `TransportManager` learns it happened.

### 2.5 Every path to a peer dies

| | |
|---|---|
| Detected by | no usable path in the peer's connection |
| Response | keep the peer in the room; keep trying candidates |
| Room | intact, that participant silent |
| User sees | that person greyed out |

Do **not** remove them from the room. They may be walking between rooms, and
removing plus re-adding forces two epoch rotations and two rounds of key
distribution for a five-second absence.

### 2.6 The relay fails

| | |
|---|---|
| Detected by | 3 missed heartbeats, independently, by every participant |
| Response | election; `RELAY_SWITCH`; media re-points |
| Room | intact, **including the key epoch** — the relay held no keys |
| User sees | ~1.5 s gap, `RelayElection` |

See [relay-election.md](relay-election.md) §6 for the worked example.

### 2.7 A participant leaves without announcing

| | |
|---|---|
| Detected by | peer timeout |
| Response | membership change, epoch advance, key material dropped |
| Room | intact |
| User sees | them disappear from the list |

Must reach the same end state as a clean departure. Recovery cannot depend on
receiving a goodbye.

### 2.8 A packet fails authentication

| | |
|---|---|
| Response | discard; **never decode** |
| Counted as | `packets_rejected_auth` |
| User sees | nothing |

Sustained non-zero rejection during an otherwise working call almost always means
a bug in nonce or epoch handling, not an attacker. Worth watching in diagnostics
for exactly that reason.

### 2.9 A packet is a replay

| | |
|---|---|
| Response | discard |
| Counted as | `packets_rejected_replay` |
| User sees | nothing |

### 2.10 Audio is interrupted (phone call, Siri, another app)

| | |
|---|---|
| Detected by | platform interruption notification |
| Response | stop capture and playback; resume on interruption end |
| Room | intact |
| User sees | the room, silent, then it comes back |

An app that does not handle this goes permanently silent after the first
interruption with no way for the user to tell why. It is not an edge case.

### 2.11 The app is backgrounded

| | |
|---|---|
| Response | keep the room alive under platform constraints; withdraw from relay candidacy |
| Room | intact where the OS permits |
| User sees | ideally nothing |

Android needs a foreground service with the microphone type; iOS needs the audio
background mode. Neither keeps Wi-Fi Aware discovery running, which is a separate
and much more restricted question.

### 2.12 The room empties

| | |
|---|---|
| Response | end the room |
| User sees | returned to the peer list |

The one case where collapsing is correct.

## 3. Anti-patterns

Things that would each look like a reasonable local fix and would each be wrong:

- **Recreating the room on a path change.** New `RoomId`, new epoch, full rekey,
  everyone's UI flashes — to solve a problem the transport layer already solved.
- **Removing a peer whose path dropped.** Two epoch rotations and O(n²) key
  deliveries for a transient.
- **Rekeying on relay change.** The relay holds no keys. Pure cost.
- **Retrying a connection inside the platform adapter.** The core scores paths and
  decides. An adapter retrying on its own hides the failure from the scorer and
  makes the metrics lie.
- **Treating "no internet" as an error.** It is Anvil's normal operating
  condition.

## 4. What to measure

Every number here is a target to be verified on real devices, not a claim:

| Metric | Target |
|---|---|
| Path switch time | < 200 ms |
| Relay recovery time | < 2 s |
| Rejoin after total loss | < 5 s |
| Concealment rate, healthy LAN | < 0.5% |
| Mouth-to-ear latency, LAN | < 150 ms |

The instrumentation for all of these exists from Phase 0, because a target you
cannot measure is a wish.
