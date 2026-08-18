import Foundation
import Network

/// LAN discovery and connectivity on iOS (§63).
///
/// `Network.framework` throughout — `NWBrowser` and `NWListener` for Bonjour
/// discovery of `_anvil._udp`, `NWConnection` for the data path.
///
/// ## Things that will cost time
///
/// **The local network permission prompt.** From iOS 14, any local network
/// access triggers a system prompt, and the app must declare
/// `NSLocalNetworkUsageDescription` plus `NSBonjourServices` listing
/// `_anvil._udp` in Info.plist. Miss either and discovery returns nothing with
/// no error. Worse, there is no API to *query* whether permission was granted —
/// the only signal is that browsing finds nothing, which is indistinguishable
/// from an empty room. Plan the UX for that ambiguity rather than discovering
/// it in testing.
///
/// **Interface pinning.** `NWParameters.requiredInterfaceType = .wifi` keeps
/// traffic off cellular. Also set `prohibitedInterfaceTypes = [.cellular]`:
/// Anvil's normal network has no internet, and iOS will otherwise consider
/// cellular the better route.
///
/// **TXT record size.** Bonjour TXT records want to stay small, which is
/// another reason the advertisement carries a fingerprint rather than a key.
///
/// PHASE1.
final class LanAdapter {

    private let emit: (PlatformEvent) -> Void
    private var browser: NWBrowser?
    private var listener: NWListener?
    private let monitor = NWPathMonitor(requiredInterfaceType: .wifi)

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
    }

    /// Whether Wi-Fi is attached.
    ///
    /// Deliberately does not require `status == .satisfied` for *internet* —
    /// a router with no WAN is a perfectly good Anvil network.
    func isAvailable() -> Bool {
        monitor.currentPath.status != .unsatisfied
    }

    /// There is no API for this. Returning true optimistically and letting
    /// discovery fail is the honest behaviour; the UI should explain the
    /// possibility when nothing is found.
    func hasLocalNetworkPermission() -> Bool { true }

    func startDiscovery() {
        // PHASE1: NWBrowser over bonjourWithTXTRecord("_anvil._udp", nil),
        // emitting PeerAdvertised per result with the TXT payload.
        fatalError("Phase 1: NWBrowser discovery")
    }

    func stopDiscovery() {
        browser?.cancel()
        browser = nil
    }

    func advertise(_ payload: Data) {
        // PHASE1: NWListener with an NWTXTRecord carrying the payload.
    }

    func stopAdvertising() {
        listener?.cancel()
        listener = nil
    }

    func connect(pathId: UInt64, address: String) {
        // PHASE1: NWConnection with QUIC parameters, interface pinned to Wi-Fi.
        // Emit PathEstablished or PathLost carrying pathId. No retry here.
        fatalError("Phase 1: LAN QUIC connect")
    }
}
