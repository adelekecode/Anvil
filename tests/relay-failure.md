# §98 — Relay failure

## Setup

- 4 devices, one room.
- Diagnostics open on all; note which device is relaying.

## Steps

1. Start a room with all four. Identify the relay in diagnostics.
2. Begin continuous conversation between two devices that are **not** the relay.
3. Kill the relay device: force-quit the app, or power it off.
4. Measure the gap before audio resumes.
5. Confirm a new relay was elected and that all three remaining devices **agree
   on which one**.
6. Bring the killed device back and rejoin.

## Expected

- Failure detected within ~1.5 s (three missed heartbeats).
- A new relay elected; audio resumes.
- **The key epoch does not advance** — the relay holds no keys, so replacing it
  costs nothing cryptographically.
- All three devices name the same new relay. This is the critical assertion: if
  they disagree, the room has split and the deterministic tie-break is broken.
- The room, its id and its remaining participants are unchanged.

## Failure signatures

| Symptom | Meaning |
|---|---|
| Devices name different relays | tie-break is not deterministic, or inputs differ between devices — the room is partitioned |
| Relay changes repeatedly afterwards | flapping; hysteresis or minimum term not applied |
| Epoch advanced | rekeying on relay change |
| Room ended | relay failure is being treated as room failure |
| A device with poor connectivity was elected | the "cannot reach everyone is disqualified" rule is not being applied |

## Also try

- Background the relay app rather than killing it — a slower, messier failure.
- Let the relay's battery run to single digits and confirm it stands down or is
  passed over.
- Kill the relay twice in quick succession and confirm the second election is not
  blocked by the minimum term.

## Record

Detection time · election time · total gap · epoch before and after ·
which device won and why (its score inputs).
