import Foundation

/// Wi-Fi Aware capability boundary on iOS.
///
/// iOS does not expose the Android-style Wi-Fi Aware/NAN API to third-party
/// apps, so claiming this capability would create a path that cannot be
/// established. The iOS LAN adapter is the supported peer-to-peer fallback:
/// its Network.framework parameters opt into AWDL peer-to-peer links while
/// keeping the Rust QUIC data plane and the protocol identical.
///
/// Android uses its real WifiAwareSession adapter. Keeping this object as an
/// explicit false capability prevents the core from trying to call a fake
/// transport on iOS and makes the platform difference visible in diagnostics.
final class WifiAwareAdapter {

    private let emit: (PlatformEvent) -> Void

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
    }

    func isAvailable() -> Bool { false }

    func startDiscovery() {
        emit(.networkChanged(kind: "wifi-aware", available: false))
    }

    func stopDiscovery() {}

    func advertise(_ payload: Data) {}

    func stopAdvertising() {}

    func connect(pathId: UInt64, address: String) {
        NSLog("Anvil: iOS has no public Wi-Fi Aware API for path \(pathId) to \(address)")
        emit(.pathLost(pathId: pathId, reason: "Wi-Fi Aware is unavailable on iOS"))
    }
}
