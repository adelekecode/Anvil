# Transport

Paths, measurement, scoring, and moving a live call from one radio to another
without anyone noticing.

## 1. The layering that makes this possible

```
  RoomSession        ← survives everything below
      │
  PeerSession        ← one per participant, survives path changes
      │
  TransportManager   ← owns candidate paths, scores them, switches
      │
  Path (LAN)   Path (Aware)   ← disposable
```

Nothing above `TransportManager` knows a path exists. That is what lets the
router be unplugged mid-sentence and the conversation continue: `RoomId`,
`PeerId`, `StreamId`, sequence state and key epochs are all untouched by the
change.

## 2. Per-peer state

```
PeerConnection
├── paths[]        every candidate, with metrics
├── active         carrying media now
├── standby        warm, scored, not carrying anything
└── active_since   for dwell-time enforcement
```

**The standby path does not receive duplicate media.** Sending every frame over
both radios doubles airtime, battery and CPU to buy redundancy that failover
already provides within a few hundred milliseconds. Selective redundancy — a
handful of important frames, or FEC over a lossy path — is a later optimisation
with a measurable case behind it.

## 3. Measurement

Cheap by design. Path monitoring must not mean continuous benchmarking: metrics
come from traffic Anvil was sending anyway (media, heartbeats, control), with
active probes reserved for a standby path that is otherwise silent.

| Metric | Source | Smoothing |
|---|---|---|
| RTT | probe replies, QUIC acks | EWMA α=0.125 |
| Jitter | arrival spacing vs send spacing | EWMA α=0.25 |
| Loss | delivered vs expected runs | EWMA α=0.10 |
| Stability | uptime since last disruption | saturates at 60 s |
| Hops | 0 direct, 1 relayed | — |
| Power | transport class | static |

Two details that matter:

- **The first real measurement replaces the placeholder** rather than blending
  with it. Blending against a made-up default makes the first few seconds lie.
- **Loss is smoothed harder than latency** because it is much noisier, and
  because a single bad second should not move a switching decision.

## 4. Scoring

Each metric normalises to 0–100, then weights:

| Metric | Weight | Normalisation |
|---|---|---|
| Loss | 0.30 | flat to 0.5%, worthless at 20% |
| Latency | 0.25 | flat to 10 ms, worthless at 300 ms |
| Jitter | 0.20 | flat to 5 ms, worthless at 150 ms |
| Stability | 0.15 | uptime/60 s, −15 per disruption |
| Hops | 0.05 | 100 / 70 / 40 |
| Power | 0.05 | LAN 80, Aware 40 |

Three decisions worth defending:

**Loss outweighs latency.** At the distances Anvil operates over, every local
path is fast. What destroys a conversation is packets going missing.

**The flat regions are deliberate.** 3 ms versus 7 ms is not a difference any
human perceives. Letting it move the score invites switching between two
indistinguishable paths.

**An unmeasured path is penalised 25 points.** Moving a live call onto a path
whose quality is still hypothetical is how you land in silence. It can win once
it has actually been probed.

LAN gets a +3 static bonus — enough to break a tie deterministically, not enough
to beat measured quality.

## 5. Switching

Three gates, in strict precedence order:

```
1. Is the active path stale (no traffic for path_timeout = 3 s)?
   └─ yes → FAILOVER now. Ignore everything below.

2. Has the active path been held for less than min_dwell (10 s)?
   └─ yes → STAY.

3. Does the candidate beat the active path by ≥ switch_hysteresis (15)?
   └─ yes → SWITCH.  no → STAY.
```

Gate 1 exists because waiting out a dwell timer while a call is silent is
indefensible. A worse *working* path beats a perfect *dead* one.

Gates 2 and 3 exist because switching is not free: packets in flight to the old
path are lost, and the receiver's playout resynchronises. Two near-equal paths
without hysteresis produce a call that ping-pongs between radios and sounds
worse than either path alone.

## 6. Heartbeats

Every 500 ms on an otherwise idle path.

Necessary because VAD means a silent participant sends no media. Without
heartbeats, a healthy path carrying a quiet person is indistinguishable from a
dead one, and the failover logic would tear down working connections every time
someone stopped talking.

## 7. QUIC

| Traffic | Mechanism | Why |
|---|---|---|
| Join, membership, keys, election | reliable stream | must arrive, in order |
| Voice, probes, heartbeats | datagram | late is worse than missing |

Putting media on a reliable stream would be a serious mistake: one lost packet
stalls every frame behind it, converting a 20 ms gap the decoder can conceal
into a multi-hundred-millisecond stall the user hears as the call breaking.

**Authentication uses raw public keys tied to the device Ed25519 identity**, not
X.509 name validation. There is no CA, no server name and no internet. A peer is
trusted because its `PeerId` matches the identity presented and what discovery
advertised.

ALPN: `anvil/1`. QUIC idle timeout 10 s — deliberately longer than Anvil's own
3 s path timeout, so Anvil notices first and can move media to a standby path,
whereas QUIC can only give up.

## 8. Datagram sizing

| Path | Conservative floor |
|---|---|
| LAN | 1200 bytes |
| Wi-Fi Aware | 1000 bytes |

Floors, raised by measurement. An Opus frame must never be split across
datagrams: one lost fragment would destroy a frame the decoder could otherwise
have concealed.

Available payload = datagram − 23 (header) − 16 (tag). At the Aware floor that
leaves 961 bytes for a ~60-byte voice frame, so this is not tight — but it will
be if a future version adds redundancy.

## 9. Platform traps

**A router is not the internet.** Anvil's normal network has no WAN. Android
marks such a network unvalidated and may route the process default over
cellular; iOS will prefer cellular for the same reason. Every socket must be
explicitly bound to the Wi-Fi network (`bindSocket` / `requiredInterfaceType`),
never the process default — which would also break the Aware path.

The failure mode is memorably confusing: discovery works, because mDNS is
link-local, and every connection times out.

**Client isolation** is common on guest and enterprise Wi-Fi. Peers reach the
gateway but not each other. The core handles it correctly — the LAN path never
reaches Ready, so Aware wins — but it deserves a distinct diagnostic rather than
a generic timeout.

## 10. Open questions

- **Do LAN and Aware interfere?** A device may not hold an Aware data path and a
  Wi-Fi association on different channels without the radio time-slicing, which
  would show as jitter on both. If measurements show it, the fix is not a special
  case: the jitter is real and the existing metric already sees it.
- **All weights and thresholds are placeholders** until measured on real devices
  (§93). The tests in `transport::scoring` assert the *relationships* — loss
  outweighs latency, near-equal paths do not switch, dead paths fail over
  immediately — so retuning the numbers cannot silently break the behaviour.
