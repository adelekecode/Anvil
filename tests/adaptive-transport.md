# §97 — Adaptive transport and failover

The most important test in the set. It is the one that proves §21 — that a room
is not a connection.

## Setup

- Router with the WAN unplugged, all devices joined to it.
- Wi-Fi Aware also available on every device.
- 3 devices minimum.
- Diagnostics open on all of them, watching the active path.

## Steps

1. Start a room. Confirm diagnostics shows **LAN active, Aware standby** on each
   device.
2. Begin a continuous conversation — one person counting aloud works well,
   because a gap is immediately obvious and its length is measurable.
3. **Cut the power to the router.**
4. Observe: how long is the gap, and does the conversation resume?
5. Keep talking for two minutes on Aware.
6. **Power the router back on**, wait for the devices to re-associate.
7. Observe whether the system switches back, and how quickly it decides.

## Expected

- Step 4: audio resumes over Aware. **The room id, participant list and key epoch
  are unchanged** — check this in diagnostics, it is the whole point.
- Target gap: under 200 ms. Anything under a second is a pass for a first run;
  record the actual number.
- Step 7: LAN is re-evaluated. It may or may not be adopted — hysteresis and the
  10 s dwell are working as designed if it stays on Aware for a while. What
  matters is that it does **not** flap between the two.

## Failure signatures

| Symptom | Meaning |
|---|---|
| Room is recreated on failover | the session is bound to the transport — a serious architectural regression |
| Participants disappear and rejoin | same |
| Key epoch advances | something is rekeying on a path change, which is pure cost |
| Rapid switching after the router returns | hysteresis or dwell time not being applied |
| No standby path existed | the same peer was probably treated as two peers — check discovery de-duplication |

## Also try

- Walk out of Wi-Fi range rather than cutting power: a slow degrade rather than a
  hard loss, which exercises the scoring path instead of the failover path.
- Saturate the LAN with other traffic and watch whether scoring notices before it
  becomes audible.

## Record

Path switch time · gap length · epoch before and after · path switches over ten
minutes (should be small).
