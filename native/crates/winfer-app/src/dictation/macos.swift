// Push-to-talk dictation on macOS: the microphone through AVAudioEngine into SFSpeechRecognizer,
// on the device when the language supports it. Built into a static library by build.rs and driven
// from `dictation/macos.rs`. One session at a time; every session ends with exactly one `ended`.

import AVFoundation
import Foundation
import Speech

/// kind: 0 text so far, 1 final text, 2 error message, 3 ended (the context can be freed).
public typealias WfDictationCallback = @convention(c) (UnsafeMutableRawPointer?, Int32, UnsafePointer<CChar>?) -> Void

private let queue = DispatchQueue(label: "dev.winfer.dictation")
private var current: Session?

private final class Session {
    let ctx: UnsafeMutableRawPointer?
    let cb: WfDictationCallback
    let engine = AVAudioEngine()
    let request = SFSpeechAudioBufferRecognitionRequest()
    var recognizer: SFSpeechRecognizer?
    var task: SFSpeechRecognitionTask?
    var text = ""
    var tapped = false
    var stopping = false
    var ended = false

    init(ctx: UnsafeMutableRawPointer?, cb: @escaping WfDictationCallback) {
        self.ctx = ctx
        self.cb = cb
    }

    func emit(_ kind: Int32, _ s: String) {
        s.withCString { cb(ctx, kind, $0) }
    }

    /// Runs on `queue`.
    func begin() {
        if stopping { return finish() }
        guard let rec = SFSpeechRecognizer(locale: Locale.current) ?? SFSpeechRecognizer(), rec.isAvailable else {
            emit(2, "Speech recognition isn't available for this language right now.")
            return finish()
        }
        recognizer = rec
        request.shouldReportPartialResults = true
        if rec.supportsOnDeviceRecognition { request.requiresOnDeviceRecognition = true }
        if #available(macOS 13, *) { request.addsPunctuation = true }
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else {
            emit(2, "No microphone found.")
            return finish()
        }
        let request = self.request
        // macOS 27 raises (an Objective-C exception, which ends the process) for buffers outside
        // 100–400 ms, so ask for 100 ms. A nil format taps the bus as it is instead of reformatting it.
        let frames = AVAudioFrameCount((format.sampleRate * 0.1).rounded(.up))
        input.installTap(onBus: 0, bufferSize: frames, format: nil) { buffer, _ in request.append(buffer) }
        tapped = true
        engine.prepare()
        do { try engine.start() } catch {
            emit(2, "Couldn't start the microphone: \(error.localizedDescription)")
            return finish()
        }
        task = rec.recognitionTask(with: request) { [weak self] result, error in
            queue.async {
                guard let self, !self.ended else { return }
                if let result {
                    self.text = result.bestTranscription.formattedString
                    self.emit(result.isFinal ? 1 : 0, self.text)
                    if result.isFinal { return self.finish() }
                }
                if let error {
                    // "No speech detected" (1110) after a release isn't worth an error; the rest is.
                    let ns = error as NSError
                    let noSpeech = ns.domain == "kAFAssistantErrorDomain" && ns.code == 1110
                    if self.text.isEmpty && !(self.stopping && noSpeech) {
                        self.emit(2, error.localizedDescription)
                    }
                    self.emit(1, self.text)
                    self.finish()
                }
            }
        }
    }

    /// Runs on `queue`: stops listening and lets the recognizer deliver its final text.
    func stop() {
        guard !stopping else { return }
        stopping = true
        guard task != nil else { return finish() }
        stopAudio()
        request.endAudio()
        // If the recognizer never answers, end with what it heard so far.
        queue.asyncAfter(deadline: .now() + 3) { [weak self] in
            guard let self, !self.ended else { return }
            self.emit(1, self.text)
            self.finish()
        }
    }

    func cancel() {
        stopping = true
        task?.cancel()
        finish()
    }

    func stopAudio() {
        if engine.isRunning { engine.stop() }
        if tapped {
            engine.inputNode.removeTap(onBus: 0)
            tapped = false
        }
    }

    func finish() {
        guard !ended else { return }
        ended = true
        stopAudio()
        task = nil
        if current === self { current = nil }
        emit(3, "")
    }
}

/// Why asking macOS for permission now could get the process killed, or nil when it's safe.
/// macOS ends a process whose request it attributes to an app without the usage texts, and it
/// attributes requests from anything started by another app (Terminal, an editor, a winfer pane
/// running a dev build) to that app. So only ask when winfer was opened as an app by macOS itself
/// (a .app whose parent is launchd) and its own Info.plist has the texts.
private func cantAsk() -> String? {
    let opened = Bundle.main.bundleURL.pathExtension == "app" && getppid() == 1
    guard opened else {
        return "macOS only asks for speech recognition and microphone access when winfer is opened as an app. Open it from Finder or the Dock; for a dev build run native/scripts/dev-app-macos.sh."
    }
    let info = Bundle.main.infoDictionary ?? [:]
    if ["NSSpeechRecognitionUsageDescription", "NSMicrophoneUsageDescription"].allSatisfy({ info[$0] != nil }) { return nil }
    return "This build of winfer can't ask for speech recognition: its Info.plist has no usage text for it. Install a newer build."
}

private let speechDenied = "Allow winfer under System Settings → Privacy & Security → Speech Recognition."
private let micDenied = "Allow winfer under System Settings → Privacy & Security → Microphone."

/// Checks the permissions, asking only for those not decided yet (and only where that's safe).
private func authorize(_ done: @escaping (String?) -> Void) {
    let speech = SFSpeechRecognizer.authorizationStatus()
    let mic = AVCaptureDevice.authorizationStatus(for: .audio)
    switch speech {
    case .denied: return done(speechDenied)
    case .restricted: return done("Speech recognition is restricted on this Mac.")
    default: break
    }
    if mic == .denied || mic == .restricted { return done(micDenied) }
    if speech == .authorized && mic == .authorized { return done(nil) }
    if let problem = cantAsk() { return done(problem) }
    SFSpeechRecognizer.requestAuthorization { status in
        guard status == .authorized else {
            return done(status == .restricted ? "Speech recognition is restricted on this Mac." : speechDenied)
        }
        AVCaptureDevice.requestAccess(for: .audio) { granted in done(granted ? nil : micDenied) }
    }
}

@_cdecl("wf_dictation_start")
public func wfDictationStart(_ ctx: UnsafeMutableRawPointer?, _ cb: WfDictationCallback) {
    queue.async {
        current?.cancel()
        let session = Session(ctx: ctx, cb: cb)
        current = session
        authorize { problem in
            queue.async {
                guard !session.ended else { return }
                if let problem {
                    session.emit(2, problem)
                    return session.finish()
                }
                session.begin()
            }
        }
    }
}

@_cdecl("wf_dictation_stop")
public func wfDictationStop() {
    queue.async { current?.stop() }
}

@_cdecl("wf_dictation_cancel")
public func wfDictationCancel() {
    queue.async { current?.cancel() }
}
