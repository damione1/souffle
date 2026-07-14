import Dispatch
import Foundation
import FoundationModels

// Compiled via build.rs on Apple Silicon macOS when the SDK ships FoundationModels.

private typealias ResponsePointer = UnsafeMutablePointer<AppleLLMResponse>

/// Inner per-request timeout. The Rust caller (summary/apple.rs) enforces a
/// 120s hard wall and abandons its waiting thread; resolving earlier on the
/// Swift side lets that thread exit normally instead of leaking.
private let requestTimeoutSeconds = 100

private func duplicateCString(_ text: String) -> UnsafeMutablePointer<CChar>? {
    text.withCString { basePointer in
        guard let duplicated = strdup(basePointer) else {
            return nil
        }
        return duplicated
    }
}

private func truncatedText(_ text: String, limit: Int) -> String {
    guard limit > 0 else { return text }
    let words = text.split(
        maxSplits: .max,
        omittingEmptySubsequences: true,
        whereSeparator: { $0.isWhitespace || $0.isNewline }
    )
    if words.count <= limit {
        return text
    }
    return words.prefix(limit).joined(separator: " ")
}

/// Stable machine-readable markers parsed by the Rust caller (summary/apple.rs)
/// to decide between retry, structural re-batching, and clean failure.
@available(macOS 26.0, *)
private func classifyGenerationError(_ error: LanguageModelSession.GenerationError) -> String {
    switch error {
    case .exceededContextWindowSize:
        return "exceeded_context_window: \(error.localizedDescription)"
    case .guardrailViolation:
        return "guardrail_violation: \(error.localizedDescription)"
    case .rateLimited:
        return "rate_limited: \(error.localizedDescription)"
    case .unsupportedLanguageOrLocale:
        return "unsupported_language: \(error.localizedDescription)"
    default:
        return error.localizedDescription
    }
}

/// Single-consumer outcome holder shared between the request task and the
/// waiting FFI thread. Once the waiter times out and abandons the request,
/// a late completion is dropped instead of racing the reader.
private final class ResultBox: @unchecked Sendable {
    private let lock = NSLock()
    private var response: String?
    private var error: String?
    private var abandoned = false

    func complete(response: String?, error: String?) {
        lock.lock()
        defer { lock.unlock() }
        guard !abandoned else { return }
        self.response = response
        self.error = error
    }

    func abandon() {
        lock.lock()
        defer { lock.unlock() }
        abandoned = true
    }

    func take() -> (response: String?, error: String?) {
        lock.lock()
        defer { lock.unlock() }
        return (response, error)
    }
}

@_cdecl("is_apple_intelligence_available")
public func isAppleIntelligenceAvailable() -> Int32 {
    guard #available(macOS 26.0, *) else {
        return 0
    }

    let model = SystemLanguageModel.default
    switch model.availability {
    case .available:
        return 1
    case .unavailable:
        return 0
    }
}

@_cdecl("apple_intelligence_unavailable_reason")
public func appleIntelligenceUnavailableReason() -> UnsafeMutablePointer<CChar>? {
    guard #available(macOS 26.0, *) else {
        return duplicateCString("macos_too_old")
    }

    let model = SystemLanguageModel.default
    switch model.availability {
    case .available:
        return nil
    case .unavailable(let reason):
        switch reason {
        case .deviceNotEligible:
            return duplicateCString("device_not_eligible")
        case .appleIntelligenceNotEnabled:
            return duplicateCString("apple_intelligence_not_enabled")
        case .modelNotReady:
            return duplicateCString("model_not_ready")
        @unknown default:
            return duplicateCString("unknown:" + String(describing: reason))
        }
    }
}

@_cdecl("process_text_with_system_prompt_apple")
public func processTextWithSystemPrompt(
    _ systemPrompt: UnsafePointer<CChar>,
    _ userContent: UnsafePointer<CChar>,
    maxTokens: Int32
) -> UnsafeMutablePointer<AppleLLMResponse> {
    let swiftSystemPrompt = String(cString: systemPrompt)
    let swiftUserContent = String(cString: userContent)
    let responsePtr = ResponsePointer.allocate(capacity: 1)
    responsePtr.initialize(to: AppleLLMResponse(response: nil, success: 0, error_message: nil))

    guard #available(macOS 26.0, *) else {
        responsePtr.pointee.error_message = duplicateCString(
            "Apple Intelligence requires macOS 26 or newer."
        )
        return responsePtr
    }

    let model = SystemLanguageModel.default
    guard model.availability == .available else {
        responsePtr.pointee.error_message = duplicateCString(
            "Apple Intelligence is not currently available on this device."
        )
        return responsePtr
    }

    let tokenLimit = max(0, Int(maxTokens))
    let semaphore = DispatchSemaphore(value: 0)
    let box = ResultBox()

    let task = Task.detached(priority: .userInitiated) {
        defer { semaphore.signal() }
        do {
            // A fresh session per request keeps every call single-turn: no
            // context accumulates across map/reduce chunks (each prompt
            // re-provides its own transcript excerpt), and one wedged
            // request cannot poison later ones through shared session state.
            let session = LanguageModelSession(
                model: model,
                instructions: swiftSystemPrompt
            )
            let generation = try await session.respond(to: swiftUserContent)
            var output = generation.content
            if tokenLimit > 0 {
                output = truncatedText(output, limit: tokenLimit)
            }
            box.complete(response: output, error: nil)
        } catch let error as LanguageModelSession.GenerationError {
            box.complete(response: nil, error: classifyGenerationError(error))
        } catch {
            box.complete(response: nil, error: error.localizedDescription)
        }
    }

    let waitResult = semaphore.wait(timeout: .now() + .seconds(requestTimeoutSeconds))
    if waitResult == .timedOut {
        // Abandon the request: drop any late completion and ask the task to
        // stop (respond may or may not honor cancellation). The Rust caller
        // treats this marker as retryable.
        box.abandon()
        task.cancel()
        responsePtr.pointee.error_message = duplicateCString(
            "request_timeout: FoundationModels did not respond within \(requestTimeoutSeconds)s"
        )
        return responsePtr
    }

    let outcome = box.take()
    if let response = outcome.response {
        responsePtr.pointee.response = duplicateCString(response)
        responsePtr.pointee.success = 1
    } else {
        responsePtr.pointee.error_message = duplicateCString(outcome.error ?? "Unknown error")
    }

    return responsePtr
}

@_cdecl("free_apple_llm_response")
public func freeAppleLLMResponse(_ response: UnsafeMutablePointer<AppleLLMResponse>?) {
    guard let response = response else { return }

    if let responseStr = response.pointee.response {
        free(UnsafeMutablePointer(mutating: responseStr))
    }

    if let errorStr = response.pointee.error_message {
        free(UnsafeMutablePointer(mutating: errorStr))
    }

    response.deallocate()
}
