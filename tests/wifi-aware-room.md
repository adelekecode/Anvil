# §96 — Wi-Fi Aware room, no router

## Setup

- **No router in range**, or all devices with Wi-Fi disconnected from any network.
- Cellular data off.
- 3–4 devices with Wi-Fi Aware support, within a few metres.
- Location services on (Aware needs it on several Android versions).

## Steps

1. Launch Anvil on all devices; confirm diagnostics reports Aware as available.
2. Wait for discovery. **Expect seconds, not milliseconds** — Aware discovery is
   duty-cycled.
3. Create a room; others join.
4. Speak in turn, confirm audio each way.
5. Hold for five minutes.
6. Walk one device slowly out of range and back; observe what happens.

## Expected

- Peers discovered with no router involved.
- Room, encryption, relay election and audio all work.
- Diagnostics shows paths as `wifi-aware`.
- The device that walks away is greyed out, **not removed from the room**, and
  rejoins the conversation when it returns without anyone re-creating anything.

## Failure signatures

| Symptom | Likely cause |
|---|---|
| Aware reported unavailable | hardware, OS version, Wi-Fi off, or location services off — check which |
| Discovery works, no data path | `WifiAwareNetworkSpecifier` misuse, or sockets not bound to the Aware network |
| Works for two devices, fails for four | Aware data path or subscriber limits — record the exact count at which it breaks |
| Battery drains fast | expected to some degree; record the number rather than being surprised |

## Cross-platform

Run Android↔Android and iOS↔iOS first, separately. **Only then** try
Android↔iOS. If the mixed case fails, that is the known Phase 5 risk, not a
regression — see `apps/mobile/ios/Runner/WifiAwareAdapter.swift` for the
fallback.

## Record

Discovery time · join time · latency · battery drain · maximum working room size.
