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
final class LanAdapter: NSObject, NetServiceDelegate {

    private let emit: (PlatformEvent) -> Void
    private var browser: NWBrowser?
    private var advertisedService: NetService?
    private let monitor = NWPathMonitor(requiredInterfaceType: .wifi)
    private let queue = DispatchQueue(label: "dev.anvil.lan")
    private var services: [String: NWEndpoint] = [:]
    private var resolvers: [String: NetService] = [:]
    private var pendingPayloads: [String: Data] = [:]

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
        super.init()
        monitor.pathUpdateHandler = { [weak self] path in
            self?.emit(.networkChanged(kind: "lan", available: path.status != .unsatisfied))
        }
        monitor.start(queue: queue)
    }

    /// Whether Wi-Fi is attached.
    ///
    /// Deliberately does not require `status == .satisfied` for *internet* —
    /// a router with no WAN is a perfectly good Anvil network.
    func isAvailable() -> Bool {
        // The monitor's initial snapshot is `.unsatisfied` until its queue has
        // delivered once. Bonjour itself is the authoritative availability
        // signal, so do not suppress the first browse during that window.
        true
    }

    /// There is no API for this. Returning true optimistically and letting
    /// discovery fail is the honest behaviour; the UI should explain the
    /// possibility when nothing is found.
    func hasLocalNetworkPermission() -> Bool { true }

    func startDiscovery() {
        guard browser == nil else { return }
        let parameters = NWParameters.udp
        parameters.includePeerToPeer = true
        parameters.prohibitedInterfaceTypes = [.cellular]

        let browser = NWBrowser(
            for: .bonjourWithTXTRecord(type: Self.serviceType, domain: nil),
            using: parameters
        )
        browser.browseResultsChangedHandler = { [weak self] _, changes in
            guard let self else { return }
            for change in changes {
                switch change {
                case let .added(result): self.found(result)
                case let .changed(_, new, _): self.found(new)
                case let .removed(result): self.lost(result)
                case .identical: break
                @unknown default: break
                }
            }
        }
        browser.stateUpdateHandler = { [weak self] state in
            switch state {
            case .ready: self?.emit(.networkChanged(kind: "lan", available: true))
            case .failed, .waiting:
                self?.emit(.networkChanged(kind: "lan", available: false))
            default: break
            }
        }
        self.browser = browser
        browser.start(queue: queue)
    }

    func stopDiscovery() {
        browser?.cancel()
        browser = nil
        let handles = Array(services.keys)
        services.removeAll()
        handles.forEach { emit(.peerAdvertisementLost(kind: "lan", handle: $0)) }
    }

    func advertise(_ payload: Data) {
        stopAdvertising()
        let service = NetService(
            domain: "local.",
            type: "\(Self.serviceType).",
            name: "",
            port: Int32(Self.quicPort)
        )
        service.delegate = self
        let encoded = Data(payload.base64EncodedString().utf8)
        service.setTXTRecord(NetService.data(fromTXTRecord: [Self.txtKey: encoded]))
        advertisedService = service
        service.publish()
    }

    func stopAdvertising() {
        advertisedService?.stop()
        advertisedService = nil
    }

    func connect(pathId: UInt64, address: String) {
        // PHASE1: NWConnection with QUIC parameters, interface pinned to Wi-Fi.
        // Emit PathEstablished or PathLost carrying pathId. No retry here.
        fatalError("Phase 1: LAN QUIC connect")
    }

    private func found(_ result: NWBrowser.Result) {
        guard case let .bonjour(record) = result.metadata,
              let encoded = record.dictionary[Self.txtKey],
              let payload = Data(base64Encoded: encoded)
        else { return }

        let handle = Self.handle(for: result.endpoint)
        services[handle] = result.endpoint
        pendingPayloads[handle] = payload
        guard case let .service(name, type, domain, _) = result.endpoint else { return }
        let resolver = NetService(domain: domain, type: type, name: name)
        resolver.delegate = self
        resolvers[handle] = resolver
        resolver.resolve(withTimeout: 5)
    }

    private func lost(_ result: NWBrowser.Result) {
        let handle = Self.handle(for: result.endpoint)
        services.removeValue(forKey: handle)
        resolvers.removeValue(forKey: handle)?.stop()
        pendingPayloads.removeValue(forKey: handle)
        emit(.peerAdvertisementLost(kind: "lan", handle: handle))
    }

    private static func handle(for endpoint: NWEndpoint) -> String {
        switch endpoint {
        case let .service(name, type, domain, _): return handle(name: name, type: type, domain: domain)
        default: return String(describing: endpoint)
        }
    }

    private static func handle(name: String, type: String, domain: String) -> String {
        [name, type, domain]
            .map { $0.trimmingCharacters(in: CharacterSet(charactersIn: ".")) }
            .joined(separator: ".")
    }

    private static let serviceType = "_anvil._udp"
    private static let txtKey = "anvil"
    private static let quicPort = 47_820

    func netServiceDidResolveAddress(_ sender: NetService) {
        let handle = Self.handle(name: sender.name, type: sender.type, domain: sender.domain)
        guard let payload = pendingPayloads[handle],
              let data = sender.addresses?.first,
              let address = Self.socketAddress(data, port: sender.port)
        else { return }
        emit(.peerAdvertised(kind: "lan", handle: handle, address: address, payload: payload))
    }

    func netService(_ sender: NetService, didNotResolve errorDict: [String: NSNumber]) {
        NSLog("Anvil: Bonjour resolve failed for \(sender.name): \(errorDict)")
    }

    private static func socketAddress(_ data: Data, port: Int) -> String? {
        data.withUnsafeBytes { raw in
            guard let base = raw.baseAddress?.assumingMemoryBound(to: sockaddr.self) else {
                return nil
            }
            var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
            guard getnameinfo(
                base,
                socklen_t(base.pointee.sa_len),
                &host,
                socklen_t(host.count),
                nil,
                0,
                NI_NUMERICHOST
            ) == 0 else { return nil }
            let value = String(cString: host)
            return value.contains(":") ? "[\(value)]:\(port)" : "\(value):\(port)"
        }
    }
}
