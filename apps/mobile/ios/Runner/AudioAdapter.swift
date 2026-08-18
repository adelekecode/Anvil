import AVFoundation

/// Microphone capture and speaker playback on iOS.
///
/// ## Session configuration
///
/// ```swift
/// try session.setCategory(
///     .playAndRecord,
///     mode: .voiceChat,              // enables echo cancellation
///     options: [.allowBluetooth, .defaultToSpeaker]
/// )
/// try session.setPreferredSampleRate(48_000)
/// try session.setPreferredIOBufferDuration(0.02)   // 20ms
/// ```
///
/// `.voiceChat` mode is what turns on the platform's echo canceller. Anvil rooms
/// are frequently speakerphone rooms — the participants are in the same physical
/// space — so without it the first thing anyone hears is feedback.
///
/// ## Interruptions are not edge cases
///
/// A phone call, a timer, Siri, or another app taking the session will all
/// interrupt audio mid-room. `AVAudioSession.interruptionNotification` must be
/// observed and forwarded as `AudioInterrupted`, and capture restarted on
/// `.ended` with the `.shouldResume` option. An app that does not handle this
/// goes permanently silent after the first interruption, and the user has no way
/// to tell why.
///
/// ## Background audio
///
/// The `audio` background mode in Info.plist keeps the session alive when the
/// app is backgrounded. It does *not* keep Wi-Fi Aware discovery running, which
/// is a separate and much more restricted question.
///
/// ## Not Anvil's transport
///
/// `.allowBluetooth` here is about the user's headset (§87). It has nothing to
/// do with which radio carries the room.
///
/// PHASE1.
final class AudioAdapter {

    private let emit: (PlatformEvent) -> Void
    private let engine = AVAudioEngine()

    init(emit: @escaping (PlatformEvent) -> Void) {
        self.emit = emit
    }

    static func hasMicrophonePermission() -> Bool {
        if #available(iOS 17.0, *) {
            return AVAudioApplication.shared.recordPermission == .granted
        }
        return AVAudioSession.sharedInstance().recordPermission == .granted
    }

    func startCapture(sampleRateHz: Int, channels: Int, frameMillis: Int) {
        // PHASE1: configure AVAudioSession, install a tap on the input node,
        // emit AudioCaptured per frame.
        fatalError("Phase 1: AVAudioEngine capture")
    }

    func stopCapture() {
        engine.inputNode.removeTap(onBus: 0)
    }

    func startPlayback(sampleRateHz: Int, channels: Int) {
        // PHASE1: AVAudioPlayerNode scheduled from the mixer output.
    }

    func stopPlayback() {
        engine.stop()
    }

    func play(_ samples: [Int16]) {
        // PHASE1: schedule a buffer. Must not block past the frame duration.
    }
}
