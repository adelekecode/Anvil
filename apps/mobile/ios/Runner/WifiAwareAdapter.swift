import Foundation

/// Wi-Fi Aware on iOS (§105).
///
/// **This is the highest-risk item in the whole plan, and it should be treated
/// as an investigation rather than an implementation task.**
///
/// Apple's peer-to-peer Wi-Fi story has historically been AWDL, exposed
/// indirectly through `Network.framework`'s `includePeerToPeer` option and
/// MultipeerConnectivity — not as a directly programmable Wi-Fi Aware / NAN
/// API of the kind Android exposes. Whether an iOS device and an Android device
/// can discover each other and establish a data path over Aware is an empirical
/// question that has to be answered on real hardware, early, before Phase 5
/// depends on it.
///
/// ## Plan for this
///
/// 1. **Answer the question first.** Before writing adapter code, build a
///    throwaway probe: iOS publishing, Android subscribing, and the reverse.
///    Two devices, one afternoon. The result determines the rest of Phase 5.
/// 2. **If interop works**, implement here against whatever API provides it,
///    keeping the same event surface as `WifiAwareAdapter.kt` so the core does
///    not learn the difference.
/// 3. **If it does not**, the fallback is already in the architecture and costs
///    no protocol change: one device hosts a local network — a personal hotspot
///    or Android's Wi-Fi Direct group — and the others join it as a *LAN* path.
///    Anvil's transport abstraction means the room, the identities, the keys and
///    the relay all work identically over that path. Only the discovery and
///    setup UX would change.
///
/// Writing this down now, rather than discovering it in Phase 5, is the
/// difference between a known trade-off and a schedule surprise.
///
/// PHASE5.
final class WifiAwareAdapter {

    private let emit: (PlatformEvent) -> Void

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
    }

    /// Conservatively false until the interop question above is answered.
    ///
    /// Reporting false means the core simply runs LAN-only, which is correct
    /// and honest, rather than advertising a path that does not work.
    func isAvailable() -> Bool { false }

    func startDiscovery() {
        // PHASE5
    }

    func stopDiscovery() {
        // PHASE5
    }

    func advertise(_ payload: Data) {
        // PHASE5
    }

    func stopAdvertising() {
        // PHASE5
    }

    func connect(pathId: UInt64, address: String) {
        // PHASE5
    }
}
