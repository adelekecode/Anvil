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
    private let player = AVAudioPlayerNode()
    private var captureInstalled = false
    private var playerAttached = false
    private var captureSamples: [Int16] = []
    private var captureTimestamp: UInt64 = 0
    private var playbackFormat: AVAudioFormat?

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
        guard !captureInstalled else { return }
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(
                .playAndRecord,
                mode: .voiceChat,
                options: [.allowBluetoothHFP, .defaultToSpeaker]
            )
            try session.setPreferredSampleRate(Double(sampleRateHz))
            try session.setPreferredIOBufferDuration(Double(frameMillis) / 1_000)
            try session.setActive(true)
        } catch {
            NSLog("Anvil: audio session setup failed: \(error)")
            return
        }

        let frameSamples = sampleRateHz * frameMillis / 1_000
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        captureSamples.removeAll(keepingCapacity: true)
        captureTimestamp = 0
        input.installTap(
            onBus: 0,
            bufferSize: AVAudioFrameCount(frameSamples),
            format: format
        ) { [weak self] buffer, _ in
            guard let self, let channelsData = buffer.floatChannelData else { return }
            let source = channelsData[0]
            for index in 0..<Int(buffer.frameLength) {
                let scaled = max(-1, min(1, source[index])) * Float(Int16.max)
                self.captureSamples.append(Int16(scaled))
            }
            while self.captureSamples.count >= frameSamples {
                let frame = Array(self.captureSamples.prefix(frameSamples))
                self.captureSamples.removeFirst(frameSamples)
                self.emit(
                    .audioCaptured(
                        samples: frame,
                        sampleRate: sampleRateHz,
                        channels: channels,
                        timestamp: self.captureTimestamp
                    )
                )
                self.captureTimestamp = (self.captureTimestamp + UInt64(frameSamples)) & 0xFFFF_FFFF
            }
        }
        captureInstalled = true
        startEngineIfNeeded()
    }

    func stopCapture() {
        guard captureInstalled else { return }
        engine.inputNode.removeTap(onBus: 0)
        captureInstalled = false
        captureSamples.removeAll()
    }

    func startPlayback(sampleRateHz: Int, channels: Int) {
        if !playerAttached {
            engine.attach(player)
            playerAttached = true
        }
        guard let format = AVAudioFormat(
            commonFormat: .pcmFormatInt16,
            sampleRate: Double(sampleRateHz),
            channels: AVAudioChannelCount(channels),
            interleaved: false
        ) else { return }
        playbackFormat = format
        engine.connect(player, to: engine.mainMixerNode, format: format)
        startEngineIfNeeded()
        if !player.isPlaying { player.play() }
    }

    func stopPlayback() {
        player.stop()
    }

    func play(_ samples: [Int16]) {
        guard let format = playbackFormat,
              let buffer = AVAudioPCMBuffer(
                pcmFormat: format,
                frameCapacity: AVAudioFrameCount(samples.count / Int(format.channelCount))
              ),
              let target = buffer.int16ChannelData
        else { return }
        buffer.frameLength = buffer.frameCapacity
        for channel in 0..<Int(format.channelCount) {
            for frame in 0..<Int(buffer.frameLength) {
                target[channel][frame] = samples[frame * Int(format.channelCount) + channel]
            }
        }
        player.scheduleBuffer(buffer)
    }

    private func startEngineIfNeeded() {
        guard !engine.isRunning else { return }
        do {
            engine.prepare()
            try engine.start()
        } catch {
            NSLog("Anvil: AVAudioEngine failed to start: \(error)")
        }
    }
}
