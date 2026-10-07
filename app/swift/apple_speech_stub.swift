import Foundation
private func unavailable() -> UnsafeMutablePointer<CChar>? { strdup("{\"available\":false,\"error\":\"Apple Speech SDK bridge is unavailable in this build\",\"reason\":\"build_unsupported\"}") }
@_cdecl("souffle_speech_status") public func speechStatus() -> UnsafeMutablePointer<CChar>? { unavailable() }
@_cdecl("souffle_speech_install") public func speechInstall(_ locale: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? { unavailable() }
@_cdecl("souffle_speech_start") public func speechStart(_ locale: UnsafePointer<CChar>) -> UnsafeMutablePointer<CChar>? { unavailable() }
@_cdecl("souffle_speech_push") public func speechPush(_ id: UInt64, _ samples: UnsafePointer<Float>?, _ count: UInt32) -> UnsafeMutablePointer<CChar>? { unavailable() }
@_cdecl("souffle_speech_finish") public func speechFinish(_ id: UInt64) -> UnsafeMutablePointer<CChar>? { unavailable() }
@_cdecl("souffle_speech_cancel") public func speechCancel(_ id: UInt64) -> UnsafeMutablePointer<CChar>? { unavailable() }
@_cdecl("souffle_speech_free") public func speechFree(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }
