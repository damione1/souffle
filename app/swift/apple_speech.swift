import Foundation
import Speech
import AVFAudio
import CoreMedia

// Calls block only the Rust engine/download workers. Task results own their
// storage across cancellation; no async closure borrows a Rust pointer.
private final class Reply: @unchecked Sendable {
    let lock = NSLock()
    var value: [String: Any] = [:]
    func store(_ value: [String: Any]) { lock.lock(); self.value = value; lock.unlock() }
    func read() -> [String: Any] { lock.lock(); defer { lock.unlock() }; return value }
}
private func response(_ value: [String: Any]) -> UnsafeMutablePointer<CChar>? {
    guard let data = try? JSONSerialization.data(withJSONObject: value),
          let string = String(data: data, encoding: .utf8) else { return strdup("{\"error\":\"serialize Speech response\"}") }
    return strdup(string)
}
private func blocking(_ seconds: Double?, _ operation: @escaping @Sendable () async throws -> [String: Any]) -> UnsafeMutablePointer<CChar>? {
    let reply = Reply()
    let done = DispatchSemaphore(value: 0)
    let task = Task {
        do { reply.store(try await operation()) }
        catch { reply.store(["error": String(describing: error)]) }
        done.signal()
    }
    let deadline = seconds.map { DispatchTime.now() + $0 } ?? .distantFuture
    guard done.wait(timeout: deadline) == .success else {
        task.cancel()
        return response(["error": "Apple Speech operation timed out"])
    }
    return response(reply.read())
}
private struct SpeechFailure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

// Rust supplies mono float32 PCM at the analyzer's negotiated rate. Speech
// assets may require another PCM representation (e.g. interleaved Int16).
// Convert representation here, without resampling or changing frame times.
private final class SpeechPCMInput {
    let format: AVAudioFormat
    private let sourceFormat: AVAudioFormat
    private let converter: AVAudioConverter?

    init(format: AVAudioFormat) throws {
        guard format.channelCount == 1,
              format.sampleRate.isFinite, format.sampleRate > 0,
              format.sampleRate <= Double(Int32.max),
              format.sampleRate.rounded() == format.sampleRate,
              let source = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: format.sampleRate, channels: 1, interleaved: false) else {
            throw SpeechFailure("Apple Speech audio format cannot accept mono float32 PCM: \(format)")
        }
        let needsConversion = !format.isEqual(source)
        let converter = needsConversion ? AVAudioConverter(from: source, to: format) : nil
        guard !needsConversion || converter != nil else {
            throw SpeechFailure("Apple Speech PCM converter unavailable")
        }
        self.format = format
        self.sourceFormat = source
        self.converter = converter
    }

    func buffer(samples: [Float]) throws -> AVAudioPCMBuffer {
        guard let count = AVAudioFrameCount(exactly: samples.count),
              let source = AVAudioPCMBuffer(pcmFormat: sourceFormat, frameCapacity: count),
              let channel = source.floatChannelData?[0] else {
            throw SpeechFailure("Speech buffer allocation failed")
        }
        source.frameLength = count
        samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: $0.count) }
        guard let converter else { return source }
        guard let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: count) else {
            throw SpeechFailure("Speech conversion buffer allocation failed")
        }
        try converter.convert(to: output, from: source)
        guard output.frameLength == count else { throw SpeechFailure("Speech PCM conversion changed frame count") }
        return output
    }
}

@available(macOS 26, *)
private actor SpeechSessions {
    static let shared = SpeechSessions()
    private struct Session {
        let analyzer: SpeechAnalyzer
        let input: AsyncStream<AnalyzerInput>.Continuation
        let task: Task<Void, Never>
        let pcm: SpeechPCMInput
        var samples: Int64 = 0
        var results: [[String: Any]] = []
        var error: String?
    }
    private var sessions: [UInt64: Session] = [:]
    private var next: UInt64 = 1

    func start(localeID: String) async throws -> [String: Any] {
        let locale = Locale(identifier: localeID)
        guard let supported = await SpeechTranscriber.supportedLocale(equivalentTo: locale) else { throw SpeechFailure("unsupported locale") }
        // Apple's live preset combines volatile revisions with a shorter
        // context window; volatileResults alone can still wait several seconds.
        let transcriber = SpeechTranscriber(locale: supported, preset: .timeIndexedProgressiveTranscription)
        guard await AssetInventory.status(forModules: [transcriber]) == .installed else { throw SpeechFailure("Apple Speech system assets are not installed") }
        guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]) else { throw SpeechFailure("Apple Speech has no installed compatible audio format") }
        let pcm = try SpeechPCMInput(format: format)
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        let (sequence, continuation) = AsyncStream<AnalyzerInput>.makeStream(bufferingPolicy: .bufferingOldest(64))
        let id = next; next += 1
        let task = Task {
            do {
                for try await result in transcriber.results {
                    let start = result.range.start.seconds
                    let end = CMTimeRangeGetEnd(result.range).seconds
                    guard start.isFinite, end.isFinite, end >= start else { throw SpeechFailure("invalid Speech timestamps") }
                    self.record(id, result: ["text": String(result.text.characters), "start_time": start, "end_time": end, "is_final": result.isFinal])
                }
            } catch { self.fail(id, error: String(describing: error)) }
        }
        sessions[id] = Session(analyzer: analyzer, input: continuation, task: task, pcm: pcm)
        do {
            try await analyzer.prepareToAnalyze(in: format)
            try await analyzer.start(inputSequence: sequence)
            try Task.checkCancellation()
            return ["session": id, "sample_rate": Int(format.sampleRate), "locale": supported.identifier]
        } catch {
            await cancel(id)
            throw error
        }
    }
    func record(_ id: UInt64, result: [String: Any]) { sessions[id]?.results.append(result) }
    func fail(_ id: UInt64, error: String) { sessions[id]?.error = error }
    func push(_ id: UInt64, samples: [Float]) async throws -> [String: Any] {
        guard let initial = sessions[id] else { throw SpeechFailure("unknown Speech session") }
        let buffer = samples.isEmpty ? nil : try initial.pcm.buffer(samples: samples)
        let deadline = ContinuousClock.now.advanced(by: .seconds(10))
        while true {
            try Task.checkCancellation()
            // Retry owns the same PCM until accepted. Re-read after every
            // suspension: result/error/cancel callbacks may have changed it.
            guard var session = sessions[id] else { throw SpeechFailure("unknown Speech session") }
            if let error = session.error { throw SpeechFailure(error) }
            if let buffer {
                guard ContinuousClock.now < deadline else {
                    throw SpeechFailure("Apple Speech input stalled; audio was not accepted")
                }
                let at = CMTime(value: session.samples, timescale: CMTimeScale(session.pcm.format.sampleRate))
                switch session.input.yield(AnalyzerInput(buffer: buffer, bufferStartTime: at)) {
                case .enqueued: session.samples += Int64(samples.count)
                case .dropped:
                    // AsyncStream rejects this value without retaining it.
                    // Backpressure the engine worker instead of losing PCM.
                    try await Task.sleep(nanoseconds: 5_000_000)
                    continue
                case .terminated: throw SpeechFailure("Apple Speech session terminated")
                @unknown default: throw SpeechFailure("unknown Speech input queue result")
                }
            }
            let results = session.results
            session.results = []
            sessions[id] = session
            return ["segments": results]
        }
    }
    func finish(_ id: UInt64) async throws -> [String: Any] {
        guard let session = sessions[id] else { throw SpeechFailure("unknown Speech session") }
        session.input.finish()
        do {
            try await session.analyzer.finalizeAndFinishThroughEndOfInput()
            await session.task.value
            try Task.checkCancellation()
            let result = try await push(id, samples: [])
            sessions.removeValue(forKey: id)
            return result
        } catch {
            await cancel(id)
            throw error
        }
    }
    func cancel(_ id: UInt64) async {
        guard let session = sessions.removeValue(forKey: id) else { return }
        session.input.finish()
        session.task.cancel()
        await session.analyzer.cancelAndFinishNow()
    }
}

@_cdecl("souffle_speech_status")
public func speechStatus() -> UnsafeMutablePointer<CChar>? {
    guard #available(macOS 26, *) else { return response(["available": false, "reason": "os_unsupported"]) }
    return blocking(10) {
        guard SpeechTranscriber.isAvailable else { return ["available": false, "reason": "device_unsupported"] }
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else { return ["available": false, "reason": "locale_unsupported"] }
        let module = SpeechTranscriber(locale: locale, preset: .transcription)
        let status = await AssetInventory.status(forModules: [module])
        let installed: Bool
        switch status {
        case .installed: installed = true
        case .supported, .downloading: installed = false
        case .unsupported: return ["available": false, "reason": "assets_unsupported"]
        @unknown default: return ["available": false, "reason": "check_failed"]
        }
        // Display names follow Soufflé's UI language; neither changes the
        // negotiated recognition locale above. Foundation owns this open set.
        let names = Dictionary(uniqueKeysWithValues: ["en", "fr"].map { language in
            (language, Locale(identifier: language).localizedString(forIdentifier: locale.identifier) ?? locale.identifier)
        })
        return ["available": true, "locale": locale.identifier,
                "locale_names": names,
                "installed": installed]
    }
}
@_cdecl("souffle_speech_install")
public func speechInstall(_ localePointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
    let localeID = String(cString: localePointer)
    guard #available(macOS 26, *) else { return response(["error": "macOS 26 is required"]) }
    return blocking(600) {
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale(identifier: localeID)) else { throw SpeechFailure("unsupported locale") }
        let module = SpeechTranscriber(locale: locale, preset: .transcription)
        // Reservation/installation happen only after explicit model selection.
        try await AssetInventory.reserve(locale: locale)
        if let request = try await AssetInventory.assetInstallationRequest(supporting: [module]) {
            try await request.downloadAndInstall()
        }
        try Task.checkCancellation()
        guard await AssetInventory.status(forModules: [module]) == .installed else { throw SpeechFailure("Apple Speech assets did not become installed") }
        return ["installed": true]
    }
}
@_cdecl("souffle_speech_start")
public func speechStart(_ localePointer: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? {
    let localeID = String(cString: localePointer)
    guard #available(macOS 26, *) else { return response(["error": "macOS 26 is required"]) }
    return blocking(30) { try await SpeechSessions.shared.start(localeID: localeID) }
}
@_cdecl("souffle_speech_push")
public func speechPush(_ session: UInt64, _ pointer: UnsafePointer<Float>?, _ count: UInt32) -> UnsafeMutablePointer<CChar>? {
    // Copy before launching Task; Rust can release/reuse its buffer on return.
    let samples: [Float] = count == 0 ? [] : Array(UnsafeBufferPointer(start: pointer, count: Int(count)))
    guard #available(macOS 26, *) else { return response(["error": "macOS 26 is required"]) }
    // Push has its own bounded, monotonic backpressure deadline. Wait for the
    // actual commit/error; an outer timeout could return before a late enqueue.
    return blocking(nil) { try await SpeechSessions.shared.push(session, samples: samples) }
}
@_cdecl("souffle_speech_finish")
public func speechFinish(_ session: UInt64) -> UnsafeMutablePointer<CChar>? {
    guard #available(macOS 26, *) else { return response(["error": "macOS 26 is required"]) }
    return blocking(30) { try await SpeechSessions.shared.finish(session) }
}
@_cdecl("souffle_speech_cancel")
public func speechCancel(_ session: UInt64) -> UnsafeMutablePointer<CChar>? {
    guard #available(macOS 26, *) else { return response(["error": "macOS 26 is required"]) }
    return blocking(10) { await SpeechSessions.shared.cancel(session); return [:] }
}
@_cdecl("souffle_speech_free")
public func speechFree(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }
