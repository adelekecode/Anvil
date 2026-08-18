import Foundation
import AVFoundation

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
        // A second attach() without a matching detach() first would retain a
        // *new* context while the adapters (lan/aware/audio/lifecycle) are
        // still wired to the old one, and would overwrite `self.sessionPtr`
        // out from under any callback already in flight against the old
        // session. Flutter hot-restart on iOS re-runs Dart's `main()` and can
        // call this a second time without the process ever exiting, so the
        // guard is not theoretical. Tear down cleanly first.
        if self.sessionPtr != nil {
            NSLog("Anvil: attach() called while already attached — detaching stale session first")
            detach()
        }
        let retained = Unmanaged.passRetained(self).toOpaque()
        var callbacks = AnvilPlatformCallbacks(
            context: retained,
            capabilities: { context in
                guard let context else { return 0 }
                return Unmanaged<AnvilPlatform>.fromOpaque(context)
                    .takeUnretainedValue().capabilitiesMask()
            },
            invoke: { context, operation, argument, bytes, length, text in
                guard let context, let operation else { return -1 }
                let platform = Unmanaged<AnvilPlatform>.fromOpaque(context).takeUnretainedValue()
                let data = bytes.map { Data(bytes: $0, count: length) } ?? Data()
                return platform.invoke(
                    operation: String(cString: operation),
                    argument: argument,
                    data: data,
                    text: text.map(String.init(cString:))
                )
            },
            load_identity: { context, buffer, capacity in
                guard context != nil, let data = AnvilKeyStore.loadIdentity() else { return 0 }
                guard let buffer else { return data.count }
                guard capacity >= data.count else { return -1 }
                data.copyBytes(to: buffer, count: data.count)
                return data.count
            },
            release: { context in
                guard let context else { return }
                Unmanaged<AnvilPlatform>.fromOpaque(context).release()
            }
        )
        let result = anvil_attach_platform(sessionPtr, &callbacks)
        if result != 0 {
            Unmanaged<AnvilPlatform>.fromOpaque(retained).release()
            NSLog("Anvil: Rust rejected Apple platform attachment: \(result)")
            return
        }
        self.sessionPtr = sessionPtr
        lifecycle.start()
    }

    func detach() {
        let session = sessionPtr
        lifecycle.stop()
        lan.stopDiscovery()
        lan.stopAdvertising()
        aware.stopDiscovery()
        audio.stopCapture()
        audio.stopPlayback()
        sessionPtr = nil
        if let session { anvil_detach_platform(session) }
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

    private func capabilitiesMask() -> UInt32 {
        let value = capabilities()
        return (value.lan ? 1 : 0)
            | (value.wifiAware ? 2 : 0)
            | (value.microphone ? 4 : 0)
            | (value.nearbyDevices ? 8 : 0)
            | (value.secureKeyStorage ? 16 : 0)
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

    private func invoke(
        operation: String,
        argument: UInt64,
        data: Data,
        text: String?
    ) -> Int32 {
        switch operation {
        case "startLanDiscovery": startLanDiscovery()
        case "stopLanDiscovery": stopLanDiscovery()
        case "startAwareDiscovery": startAwareDiscovery()
        case "stopAwareDiscovery": stopAwareDiscovery()
        case "advertise": advertise(payload: data)
        case "stopAdvertising": stopAdvertising()
        case "connectLan": connect(pathId: argument, kind: "lan", address: text ?? "")
        case "connectAware": connect(pathId: argument, kind: "wifi-aware", address: text ?? "")
        case "startCapture":
            let parts = (text ?? "1,20").split(separator: ",").compactMap { Int($0) }
            audio.startCapture(
                sampleRateHz: Int(argument),
                channels: parts.first ?? 1,
                frameMillis: parts.dropFirst().first ?? 20
            )
        case "stopCapture": audio.stopCapture()
        case "startPlayback":
            audio.startPlayback(sampleRateHz: Int(argument), channels: Int(text ?? "1") ?? 1)
        case "stopPlayback": audio.stopPlayback()
        case "play":
            let samples = data.withUnsafeBytes { raw -> [Int16] in
                Array(raw.bindMemory(to: Int16.self))
            }
            audio.play(samples)
        case "storeIdentity": AnvilKeyStore.storeIdentity(data)
        case "clearIdentity": AnvilKeyStore.clearIdentity()
        case "requestPermission": requestPermission(text ?? "")
        case "listen": break // NWListener is established by advertise().
        default:
            NSLog("Anvil: unsupported native operation \(operation)")
            return -2
        }
        return 0
    }

    private func requestPermission(_ capability: String) {
        switch capability {
        case "microphone":
            AVAudioSession.sharedInstance().requestRecordPermission { [weak self] granted in
                self?.emit(.permissionChanged(capability: capability, granted: granted))
            }
        case "nearby_devices":
            // iOS exposes no explicit local-network permission API. Starting a
            // declared Bonjour browse is the system-supported prompt trigger.
            startLanDiscovery()
            emit(.permissionChanged(capability: capability, granted: true))
        default:
            emit(.permissionChanged(capability: capability, granted: false))
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
