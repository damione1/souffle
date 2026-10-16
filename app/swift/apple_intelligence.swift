import Dispatch
import Foundation
import FoundationModels

// Compiled via build.rs on Apple Silicon macOS when the SDK ships FoundationModels.

private typealias ResponsePointer = UnsafeMutablePointer<AppleLLMResponse>

private func duplicateCString(_ text: String) -> UnsafeMutablePointer<CChar>? {
    text.withCString { basePointer in
        guard let duplicated = strdup(basePointer) else {
            return nil
        }
        return duplicated
    }
}

/// Stable machine-readable markers parsed by the Rust caller (summary/apple.rs)
/// to decide between retry, structural re-batching, and clean failure.
@available(macOS 26.0, *)
private func classifyGenerationError(_ error: LanguageModelSession.GenerationError) -> String {
    switch error {
    case .exceededContextWindowSize:
        return "exceeded_context_window: \(error.localizedDescription)"
    case .guardrailViolation, .refusal:
        return "guardrail_violation: \(error.localizedDescription)"
    case .rateLimited:
        return "rate_limited: \(error.localizedDescription)"
    case .unsupportedLanguageOrLocale:
        return "unsupported_language: \(error.localizedDescription)"
    default:
        return classifyFallbackError(error)
    }
}

private func classifyFallbackError(_ error: any Error) -> String {
    let description = error.localizedDescription
    let reflected = String(describing: error)
    let lower = (description + " " + reflected).lowercased()
    if lower.contains("contextsizeexceeded") || lower.contains("exceededcontextwindowsize")
        || lower.contains("context window")
    {
        return "exceeded_context_window: \(description)"
    }
    if lower.contains("guardrail") || lower.contains("refusal") {
        return "guardrail_violation: \(description)"
    }
    if lower.contains("ratelimited") || lower.contains("rate limit") {
        return "rate_limited: \(description)"
    }
    if lower.contains("unsupportedlanguage") || lower.contains("unsupported locale") {
        return "unsupported_language: \(description)"
    }
    return description
}

/// Single-consumer outcome holder shared between the request task and the
/// waiting FFI thread inside the disposable helper subprocess.
private final class ResultBox: @unchecked Sendable {
    private let lock = NSLock()
    private var response: String?
    private var error: String?

    func complete(response: String?, error: String?) {
        lock.lock()
        defer { lock.unlock() }
        self.response = response
        self.error = error
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
    temperature: Float,
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

    // Use `.permissiveContentTransformations` so summarizing, extracting, or
    // polishing user-supplied meeting transcripts does not trip open-ended
    // generative safety guardrails on sensitive discussion topics.
    let model = SystemLanguageModel(
        useCase: .general,
        guardrails: .permissiveContentTransformations
    )
    guard model.availability == .available else {
        responsePtr.pointee.error_message = duplicateCString(
            "Apple Intelligence is not currently available on this device."
        )
        return responsePtr
    }

    let tokenLimit = max(0, Int(maxTokens))
    let requestedTemperature: Double? =
        temperature.isFinite && temperature >= 0 ? Double(temperature) : nil
    let semaphore = DispatchSemaphore(value: 0)
    let box = ResultBox()

    Task.detached(priority: .userInitiated) {
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
            let options = GenerationOptions(
                temperature: requestedTemperature,
                maximumResponseTokens: tokenLimit > 0 ? tokenLimit : nil
            )
            let generation = try await session.respond(
                to: swiftUserContent,
                options: options
            )
            box.complete(response: generation.content, error: nil)
        } catch let error as LanguageModelSession.GenerationError {
            box.complete(response: nil, error: classifyGenerationError(error))
        } catch {
            box.complete(response: nil, error: classifyFallbackError(error))
        }
    }

    // The parent Rust supervisor (`run_helper_process`) enforces the caller's
    // wall-clock deadline and kills/reaps this disposable helper subprocess on
    // timeout.
    semaphore.wait()

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
