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
        guard observers.isEmpty else { return }
        UIDevice.current.isBatteryMonitoringEnabled = true
        let center = NotificationCenter.default
        observers.append(center.addObserver(
            forName: UIApplication.didEnterBackgroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in self?.emit(.lifecycleChanged(foreground: false)) })
        observers.append(center.addObserver(
            forName: UIApplication.willEnterForegroundNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in self?.emit(.lifecycleChanged(foreground: true)) })
        observers.append(center.addObserver(
            forName: UIDevice.batteryLevelDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in self?.emitStatus() })
        observers.append(center.addObserver(
            forName: UIDevice.batteryStateDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in self?.emitStatus() })
        if #available(iOS 11.0, *) {
            observers.append(center.addObserver(
                forName: ProcessInfo.thermalStateDidChangeNotification,
                object: nil,
                queue: .main
            ) { [weak self] _ in self?.emitStatus() })
        }
        emit(.lifecycleChanged(foreground: true))
        emitStatus()
    }

    func stop() {
        observers.forEach(NotificationCenter.default.removeObserver)
        observers.removeAll()
        UIDevice.current.isBatteryMonitoringEnabled = false
    }

    private func emitStatus() {
        let level = UIDevice.current.batteryLevel
        let percentage: Int? = level >= 0 ? Int((level * 100).rounded()) : nil
        let state = UIDevice.current.batteryState
        let charging = state == .charging || state == .full
        let throttled: Bool
        if #available(iOS 11.0, *) {
            throttled = ProcessInfo.processInfo.thermalState == .serious ||
                ProcessInfo.processInfo.thermalState == .critical
        } else {
            throttled = false
        }
        emit(.deviceStatus(
            batteryPct: percentage,
            charging: charging,
            thermallyThrottled: throttled
        ))
    }
}
