# Relay election

## 1. Why there is a relay at all

Full mesh needs N(N−1)/2 connections. For four people that is six; for eight it
is twenty-eight, and each participant is encoding and uploading N−1 streams over
one radio. Phones do not have the uplink, the CPU or the battery for that.

So group rooms route media through one elected participant:

```
        Bob
         │
 Alice ─ R ─ Chris          Alice uploads 1 stream.
         │                  R forwards it to 3.
       David
```

Two-person rooms use no relay: inserting a hop between two people costs latency
and battery for no fan-out benefit.

## 2. What the relay is not

It is a room node doing a job. It holds no media keys, decides no membership,
and gets no cryptographic capability from the role. If the relay device belongs
to a participant — which it always does in v0.1 — that person can hear the room
because they are a participant.

**Exactly two packet types are relayable: `MEDIA` and `HEARTBEAT`.** Everything
else is refused, and that is enforced by test. A relay that could forward
`MEMBERSHIP`, `KEY_EXCHANGE` or `RELAY_SWITCH` would have a hand in who is in the
room, who holds keys, and its own succession.

Heartbeats are relayable because VAD means a silent participant sends no media;
without them a quiet person looks dead.

## 3. Candidacy

Each participant advertises its own suitability:

| Input | Weight | Notes |
|---|---|---|
| Connectivity | 0.40 | fraction of members reachable directly |
| Network quality | 0.25 | mean path score to those members |
| Stability | 0.20 | path uptime, disruption history |
| Capability | 0.15 | CPU and thermal headroom |
| Charging | +5 flat | on external power |
| Battery penalty | −15 / −40 | below 40% / below 15% |

**A candidate that cannot reach every member scores zero.** Not penalised —
disqualified. Electing it partitions the room, which is worse than electing a
slow relay.

**A charging device wins ties.** Relaying is the most expensive job in the room
and a plugged-in phone pays nothing for it.

**A nearly flat phone can still serve if it is the only candidate.** Losing the
room is worse than relaying at 6%. There is a `hard_battery_floor` setting for
deployments that disagree.

### 3.1 Scores are self-reported, and that is fine

A malicious device can claim a perfect score to capture the role. Worth stating
plainly rather than hiding.

What it buys: the ability to drop, delay and reorder packets, and to observe
metadata — nearly all of which it could do as an ordinary participant anyway.

What it does not buy: any key material whatsoever. A liar becomes a bad relay,
health monitoring notices, and an election removes them. The cost of lying is
bounded by the trust model, which is why the trust model is the load-bearing
part and the scoring is not.

## 4. Elections

```
elect(candidates, incumbent, incumbent_since, incumbent_failed, members, now)
```

Precedence:

1. **No incumbent** → best candidate wins (`Bootstrap`).
2. **Incumbent failed** → best candidate wins immediately, ignoring term and
   hysteresis (`RelayFailed`).
3. **Inside `min_term` (30 s)** → no change.
4. **Challenger beats incumbent by ≥ `election_hysteresis` (20)** → change
   (`BetterCandidate`).
5. Otherwise → no change.

Ties break by `PeerId` ordering.

### 4.1 Flapping is the real enemy

Changing relay is expensive: every participant re-points its media, packets in
flight to the old relay are lost, and the room glitches. Two devices with
near-identical scores swapping the role every few seconds would produce a call
audibly worse than either one relaying badly.

Hence three defences that must all hold: hysteresis (wider than the transport
layer's, because the switch costs more), a minimum term, and deterministic
tie-breaks.

### 4.2 Determinism is not a nicety

Every participant runs the election independently. If two nodes with equal scores
each conclude they won, the room splits and neither half hears the other. That is
why ties break on `PeerId` and why room membership is held in a `BTreeMap` — so
every device iterates in the same order and reaches the same answer.

## 5. Health

Every participant watches the relay independently. Nobody is in charge of
declaring failure, because there is nobody to be in charge — and the node most
likely to notice first is the one the relay stopped talking to.

Detection is by **missed heartbeats**, not absence of media. A healthy room can be
completely silent for a minute; a relay that is quiet because nobody is speaking
must not be mistaken for one that walked out of range.

| Missed (500 ms each) | Health | Action |
|---|---|---|
| 0 | Healthy | — |
| 1–2 | Degraded | UI may warn; protocol does nothing |
| 3+ | Failed | election |

`Degraded` exists so the UI has something honest to say during the second or two
before a failover, instead of unexplained silence.

## 6. Failure, worked through

```
        Alice                        Alice
          │            Chris           │
 Bob ─── Chris ─── David   ──►   Bob ─────── David
        RELAY                    RELAY
```

1. Chris disappears — backgrounded, out of range, battery dead.
2. Alice, Bob and David each miss three heartbeats, independently.
3. Each runs an election. All reach the same answer, because the inputs are
   shared and the tie-break is deterministic.
4. `RELAY_SWITCH` commits the result; media re-points.
5. Audio resumes. **The room, the membership and the key epoch are unchanged** —
   nobody rejoined anything, and no keys rotated, because the relay held none.

Target recovery time: under two seconds, dominated by the three missed
heartbeats. Measured, not assumed.

## 7. Unpinned

- **Election message flow.** Whether `RELAY_ELECTION` is a single round of
  votes, or whether each node simply computes and announces, with `RELAY_SWITCH`
  from the winner as the commit. The latter is simpler and probably sufficient
  given deterministic inputs; it needs writing down before Phase 3.
- **Term numbering.** How terms are allocated and how a stale announcement from a
  previous term is recognised.
- **Relay resignation.** A relay that is backgrounding or running flat should
  stand down gracefully rather than being detected as failed. Cheaper for
  everyone, but needs a message and a hand-over rule.
