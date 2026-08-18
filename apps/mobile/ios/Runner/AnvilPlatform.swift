import Foundation

/// The iOS side of the platform boundary (§10, §90).
///
/// Mirrors `AnvilPlatform.kt` exactly. That symmetry is the point: the Rust core
/// has one code path, and any behavioural difference between platforms should
/// come from the OS, never from the adapters disagreeing about their job.
///
/// As on Android, this layer exposes capabilities and pushes events. It does not
/// choose transports, retry connections, de-duplicate peers, or touch media
/// timing.
///
/// ## Phase status
///
/// Phase 0: structure and the bridge. Phases 1 and 5 fill it in.
final class AnvilPlatform {

    private lazy var lan = LanAdapter { [weak self] in self?.emit($0) }
    private lazy var aware = WifiAwareAdapter { [weak self] in self?.emit($0) }
    private lazy var audio = AudioAdapter { [weak self] in self?.emit($0) }
    private lazy var lifecycle = LifecycleAdapter { [weak self] in self?.emit($0) }

    /// Native session pointer, set after `anvil_init`.
    private var sessionPtr: UnsafeMutableRawPointer?

    func attach(sessionPtr: UnsafeMutableRawPointer) {
        self.sessionPtr = sessionPtr
        lifecycle.start()
    }

    func detach() {
        lifecycle.stop()
        lan.stopDiscovery()
        aware.stopDiscovery()
        audio.stopCapture()
        audio.stopPlayback()
        sessionPtr = nil
    }

    /// What this device can do right now.
    ///
    /// Re-queried on every network change: Wi-Fi Aware availability is not
    /// static, and a device that had it a minute ago may not now.
    func capabilities() -> Capabilities {
        Capabilities(
            lan: lan.isAvailable(),
            wifiAware: aware.isAvailable(),
            microphone: AudioAdapter.hasMicrophonePermission(),
            nearbyDevices: lan.hasLocalNetworkPermission(),
            secureKeyStorage: AnvilKeyStore.hasSecureEnclave()
        )
    }

    // MARK: - Calls from the core

    func startLanDiscovery() { lan.startDiscovery() }
    func stopLanDiscovery() { lan.stopDiscovery() }
    func startAwareDiscovery() { aware.startDiscovery() }
    func stopAwareDiscovery() { aware.stopDiscovery() }

    func advertise(payload: Data) {
        lan.advertise(payload)
        aware.advertise(payload)
    }

    func stopAdvertising() {
        lan.stopAdvertising()
        aware.stopAdvertising()
    }

    func connect(pathId: UInt64, kind: String, address: String) {
        switch kind {
        case "lan": lan.connect(pathId: pathId, address: address)
        case "wifi-aware": aware.connect(pathId: pathId, address: address)
        default: NSLog("Anvil: unknown path kind \(kind)")
        }
    }

    // MARK: - Events to the core

    private func emit(_ event: PlatformEvent) {
        guard let sessionPtr else { return }
        event.jsonString().withCString { json in
            let result = anvil_submit_platform_event(sessionPtr, json)
            if result != 0 {
                NSLog("Anvil: platform event rejected by core: \(result)")
            }
        }
    }
}

/// Mirrors `anvil_core::platform::Capabilities`.
struct Capabilities {
    let lan: Bool
    let wifiAware: Bool
    let microphone: Bool
    let nearbyDevices: Bool
    let secureKeyStorage: Bool
}

/// Declared in the Rust static library. PHASE1.
@_silgen_name("anvil_submit_platform_event")
func anvil_submit_platform_event(
    _ session: UnsafeMutableRawPointer,
    _ json: UnsafePointer<CChar>
) -> Int32
