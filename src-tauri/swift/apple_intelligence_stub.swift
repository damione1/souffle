import Foundation

// Stub implementation when FoundationModels is unavailable at build time.

private typealias ResponsePointer = UnsafeMutablePointer<AppleLLMResponse>

@_cdecl("is_apple_intelligence_available")
public func isAppleIntelligenceAvailable() -> Int32 {
    0
}

@_cdecl("apple_intelligence_unavailable_reason")
public func appleIntelligenceUnavailableReason() -> UnsafeMutablePointer<CChar>? {
    strdup("stub")
}

@_cdecl("process_text_with_system_prompt_apple")
public func processTextWithSystemPrompt(
    _ systemPrompt: UnsafePointer<CChar>,
    _ userContent: UnsafePointer<CChar>,
    maxTokens: Int32
) -> UnsafeMutablePointer<AppleLLMResponse> {
    let responsePtr = ResponsePointer.allocate(capacity: 1)
    responsePtr.initialize(to: AppleLLMResponse(response: nil, success: 0, error_message: nil))

    let msg = "Apple Intelligence is not available in this build."
    responsePtr.pointee.error_message = strdup(msg)

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
