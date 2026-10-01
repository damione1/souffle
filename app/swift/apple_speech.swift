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
private func blocking(_ seconds: Double, _ operation: @escaping @Sendable () async throws -> [String: Any]) -> UnsafeMutablePointer<CChar>? {
    let reply = Reply()
    let done = DispatchSemaphore(value: 0)
    let task = Task {
        do { reply.store(try await operation()) }
        catch { reply.store(["error": String(describing: error)]) }
        done.signal()
    }
    guard done.wait(timeout: .now() + seconds) == .success else {
        task.cancel()
        return response(["error": "Apple Speech operation timed out"])
    }
    return response(reply.read())
}
private struct SpeechFailure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

@available(macOS 26, *)
private actor SpeechSessions {
    static let shared = SpeechSessions()
    private struct Session {
        let analyzer: SpeechAnalyzer
        let input: AsyncStream<AnalyzerInput>.Continuation
        let task: Task<Void, Never>
        let format: AVAudioFormat
        var samples: Int64 = 0
        var results: [[String: Any]] = []
        var error: String?
    }
    private var sessions: [UInt64: Session] = [:]
    private var next: UInt64 = 1

    func start(localeID: String) async throws -> [String: Any] {
        let locale = Locale(identifier: localeID)
        guard let supported = await SpeechTranscriber.supportedLocale(equivalentTo: locale) else { throw SpeechFailure("unsupported locale") }
        let transcriber = SpeechTranscriber(locale: supported, transcriptionOptions: [], reportingOptions: [], attributeOptions: [.audioTimeRange])
        guard await AssetInventory.status(forModules: [transcriber]) == .installed else { throw SpeechFailure("Apple Speech system assets are not installed") }
        guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [transcriber]), format.commonFormat == .pcmFormatFloat32, !format.isInterleaved, format.channelCount == 1 else { throw SpeechFailure("Apple Speech has no compatible float32 mono audio format") }
        let analyzer = SpeechAnalyzer(modules: [transcriber])
        let (sequence, continuation) = AsyncStream<AnalyzerInput>.makeStream(bufferingPolicy: .bufferingOldest(64))
        let id = next; next += 1
        let task = Task {
            do {
                for try await result in transcriber.results {
                    let start = result.range.start.seconds
                    let end = CMTimeRangeGetEnd(result.range).seconds
                    guard start.isFinite, end.isFinite, end >= start else { throw SpeechFailure("invalid Speech timestamps") }
                    self.record(id, result: ["text": String(result.text.characters), "start_time": start, "end_time": end])
                }
            } catch { self.fail(id, error: String(describing: error)) }
        }
        sessions[id] = Session(analyzer: analyzer, input: continuation, task: task, format: format)
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
    func push(_ id: UInt64, samples: [Float]) throws -> [String: Any] {
        guard var session = sessions[id] else { throw SpeechFailure("unknown Speech session") }
        if let error = session.error { throw SpeechFailure(error) }
        if !samples.isEmpty {
            guard let buffer = AVAudioPCMBuffer(pcmFormat: session.format, frameCapacity: AVAudioFrameCount(samples.count)), let channel = buffer.floatChannelData?[0] else { throw SpeechFailure("Speech buffer allocation failed") }
            buffer.frameLength = AVAudioFrameCount(samples.count)
            samples.withUnsafeBufferPointer { channel.update(from: $0.baseAddress!, count: $0.count) }
            let at = CMTime(value: session.samples, timescale: CMTimeScale(session.format.sampleRate))
            switch session.input.yield(AnalyzerInput(buffer: buffer, bufferStartTime: at)) {
            case .enqueued: break
            case .dropped: throw SpeechFailure("Apple Speech input queue overflow; audio was not accepted")
            case .terminated: throw SpeechFailure("Apple Speech session terminated")
            @unknown default: throw SpeechFailure("unknown Speech input queue result")
            }
            session.samples += Int64(samples.count)
        }
        let results = session.results
        session.results = []
        sessions[id] = session
        return ["segments": results]
    }
    func finish(_ id: UInt64) async throws -> [String: Any] {
        guard let session = sessions[id] else { throw SpeechFailure("unknown Speech session") }
        session.input.finish()
        do {
            try await session.analyzer.finalizeAndFinishThroughEndOfInput()
            await session.task.value
            try Task.checkCancellation()
            let result = try push(id, samples: [])
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
    guard #available(macOS 26, *) else { return response(["available": false, "reason": "macOS 26 is required"]) }
    return blocking(10) {
        guard SpeechTranscriber.isAvailable else { return ["available": false, "reason": "SpeechTranscriber is unavailable on this device"] }
        guard let locale = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current) else { return ["available": false, "reason": "System locale is not supported by Apple Speech"] }
        let module = SpeechTranscriber(locale: locale, preset: .transcription)
        let status = await AssetInventory.status(forModules: [module])
        let installed: Bool
        switch status {
        case .installed: installed = true
        case .supported, .downloading: installed = false
        case .unsupported: return ["available": false, "reason": "Apple Speech assets are unsupported for this locale"]
        @unknown default: return ["available": false, "reason": "Unknown system Speech asset status"]
        }
        return ["available": true, "locale": locale.identifier, "installed": installed]
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
    return blocking(10) { try await SpeechSessions.shared.push(session, samples: samples) }
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
