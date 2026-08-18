import Flutter
import UIKit

@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  private let anvilPlatform = AnvilPlatform()
  private var anvilChannel: FlutterMethodChannel?

  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }

  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)

    let channel = FlutterMethodChannel(
      name: "dev.anvil/platform",
      binaryMessenger: engineBridge.applicationRegistrar.messenger()
    )
    anvilChannel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return }
      switch call.method {
      case "attach":
        guard
          let arguments = call.arguments as? [String: Any],
          let number = arguments["session"] as? NSNumber,
          let pointer = UnsafeMutableRawPointer(bitPattern: UInt(number.uint64Value))
        else {
          result(FlutterError(
            code: "invalid_session",
            message: "Missing native session pointer",
            details: nil
          ))
          return
        }
        self.anvilPlatform.attach(sessionPtr: pointer)
        result(nil)
      case "detach":
        self.anvilPlatform.detach()
        result(nil)
      default:
        result(FlutterMethodNotImplemented)
      }
    }
  }
}
