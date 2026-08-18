# Device test plans

Automated tests live with the code: `cargo test --workspace` runs 222 of them and
needs no device, no network and no real time. Path failover, relay election,
replay rejection and jitter buffering are all covered there, because the clock
and the platform are injected.

**This directory is for the tests that cannot work that way** — the ones that
need real phones, real radios and a router someone can physically unplug. They
are the scenarios from the architecture spec, §95–§99.

Each plan states its setup, the exact steps, what should happen, and — more
usefully — what failure looks like, so that a run that goes wrong produces a
useful bug report rather than "it didn't work".

| Plan | Covers | Needs |
|---|---|---|
| [lan-room.md](lan-room.md) | §95 router test | 3–4 devices, a router, no internet |
| [wifi-aware-room.md](wifi-aware-room.md) | §96 Aware test | 3–4 supporting devices, no router |
| [adaptive-transport.md](adaptive-transport.md) | §97 failover | both of the above at once |
| [relay-failure.md](relay-failure.md) | §98 relay loss | 4 devices |
| [security.md](security.md) | §99 capture, tamper, replay | 3 devices + a capture host |

Record results with the app's diagnostics view open. Every number in §93 —
mouth-to-ear latency, join time, relay recovery time, path switch time — comes
from these runs, and none of them should be quoted from anywhere else.
