import UIKit

/// App lifecycle and device status.
///
/// Foreground/background matters because iOS restricts radio and microphone
/// access in the background, and because a backgrounded device makes a poor
/// relay however good its network looks.
///
/// Battery and charging state feed relay election (§37). Note
/// `UIDevice.current.isBatteryMonitoringEnabled` must be set before battery
/// level reads anything but -1 — a small thing that silently produces a
/// nonsense relay score if forgotten.
///
/// PHASE1.
final class LifecycleAdapter {

    private let emit: (PlatformEvent) -> Void
    private var observers: [NSObjectProtocol] = []

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
    }

    func start() {
        UIDevice.current.isBatteryMonitoringEnabled = true
        // PHASE1: observe didEnterBackgroundNotification,
        // willEnterForegroundNotification, batteryLevelDidChangeNotification,
        // batteryStateDidChangeNotification and thermalStateDidChangeNotification.
    }

    func stop() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        UIDevice.current.isBatteryMonitoringEnabled = false
    }
}
