import Foundation

/// Events pushed to the Rust core, normalised to be identical to the Android
/// adapter's output (§10, §90).
///
/// The field names here and in `PlatformEvent.kt` must match exactly — the core
/// has one parser.
enum PlatformEvent {

    case peerAdvertised(kind: String, handle: String, address: String, payload: Data)
    case peerAdvertisementLost(kind: String, handle: String)
    case pathEstablished(pathId: UInt64, maxDatagramSize: Int)
    case pathLost(pathId: UInt64, reason: String)
    case networkChanged(kind: String, available: Bool)
    case audioInterrupted(resumed: Bool)
    case audioRouteChanged(route: String)
    case permissionChanged(capability: String, granted: Bool)
    case deviceStatus(batteryPct: Int?, charging: Bool, thermallyThrottled: Bool)
    case lifecycleChanged(foreground: Bool)

    func jsonString() -> String {
        let object: [String: Any]

        switch self {
        case let .peerAdvertised(kind, handle, address, payload):
            object = [
                "type": "peerAdvertised",
                "kind": kind,
                "handle": handle,
                "address": address,
                "payload": [UInt8](payload).map { Int($0) },
            ]
        case let .peerAdvertisementLost(kind, handle):
            object = ["type": "peerAdvertisementLost", "kind": kind, "handle": handle]
        case let .pathEstablished(pathId, maxDatagramSize):
            object = [
                "type": "pathEstablished",
                "pathId": pathId,
                "maxDatagramSize": maxDatagramSize,
            ]
        case let .pathLost(pathId, reason):
            object = ["type": "pathLost", "pathId": pathId, "reason": reason]
        case let .networkChanged(kind, available):
            object = ["type": "networkChanged", "kind": kind, "available": available]
        case let .audioInterrupted(resumed):
            object = ["type": "audioInterrupted", "resumed": resumed]
        case let .audioRouteChanged(route):
            object = ["type": "audioRouteChanged", "route": route]
        case let .permissionChanged(capability, granted):
            object = [
                "type": "permissionChanged",
                "capability": capability,
                "granted": granted,
            ]
        case let .deviceStatus(batteryPct, charging, thermallyThrottled):
            object = [
                "type": "deviceStatus",
                "batteryPct": batteryPct as Any,
                "charging": charging,
                "thermallyThrottled": thermallyThrottled,
            ]
        case let .lifecycleChanged(foreground):
            object = ["type": "lifecycleChanged", "foreground": foreground]
        }

        guard let data = try? JSONSerialization.data(withJSONObject: object),
              let text = String(data: data, encoding: .utf8)
        else {
            return "{\"type\":\"unknown\"}"
        }
        return text
    }
}
