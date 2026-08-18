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
    private let queue = DispatchQueue(label: "dev.anvil.lan")
    private var services: [String: NWEndpoint] = [:]

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
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
        let handles = services.keys
        services.removeAll()
        handles.forEach { emit(.peerAdvertisementLost(kind: "lan", handle: $0)) }
    }

    func advertise(_ payload: Data) {
        stopAdvertising()
        let parameters = NWParameters.udp
        parameters.includePeerToPeer = true
        parameters.prohibitedInterfaceTypes = [.cellular]
        do {
            let listener = try NWListener(using: parameters)
            listener.service = NWListener.Service(
                name: nil,
                type: Self.serviceType,
                domain: nil,
                txtRecord: NWTXTRecord([Self.txtKey: payload.base64EncodedString()])
            )
            listener.newConnectionHandler = { connection in
                // The transport layer adopts inbound connections in the next
                // step; rejecting here is preferable to silently retaining one.
                connection.cancel()
            }
            listener.stateUpdateHandler = { state in
                if case let .failed(error) = state {
                    NSLog("Anvil: Bonjour listener failed: \(error)")
                }
            }
            self.listener = listener
            listener.start(queue: queue)
        } catch {
            NSLog("Anvil: could not advertise Bonjour service: \(error)")
        }
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

    private func found(_ result: NWBrowser.Result) {
        guard case let .bonjour(record) = result.metadata,
              let encoded = record.dictionary[Self.txtKey],
              let payload = Data(base64Encoded: encoded)
        else { return }

        let handle = Self.handle(for: result.endpoint)
        services[handle] = result.endpoint
        emit(
            .peerAdvertised(
                kind: "lan",
                handle: handle,
                address: handle,
                payload: payload
            )
        )
    }

    private func lost(_ result: NWBrowser.Result) {
        let handle = Self.handle(for: result.endpoint)
        services.removeValue(forKey: handle)
        emit(.peerAdvertisementLost(kind: "lan", handle: handle))
    }

    private static func handle(for endpoint: NWEndpoint) -> String {
        switch endpoint {
        case let .service(name, type, domain, _): return "\(name).\(type).\(domain)"
        default: return String(describing: endpoint)
        }
    }

    private static let serviceType = "_anvil._udp"
    private static let txtKey = "anvil"
}
