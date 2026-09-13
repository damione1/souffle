import type { RecordingKind } from "../types";
import { assertNever } from "./exhaustive";

/** Which kind of session a `RecordingKind` describes.
 *
 * `RecordingKind` is a tagged union in Rust (`"dictation"` or
 * `{ meeting: ... }`), so it is read by its tag rather than sniffed with
 * `typeof x === "object"`. A third variant stops compiling here instead of
 * being classified by its shape. */
export function recordingKindMode(kind: RecordingKind): "dictation" | "meeting" {
  if (kind === "dictation") return "dictation";
  if ("meeting" in kind) return "meeting";
  return assertNever(kind, "RecordingKind");
}
