import type { Speaker } from "../types";

/** Parsed result of a speaker label: Me (microphone) or Them (system audio). */
export type SpeakerLabel = { kind: "me" } | { kind: "them" };

/** Resolve a segment/paragraph's `speaker` value. `null`/`undefined` and
 * anything other than "me"/"them" (including leftover `spk:<id>` labels)
 * resolve to `null` — callers should just not render a badge. */
export function resolveSpeakerLabel(
  speaker: Speaker | null | undefined,
): SpeakerLabel | null {
  if (speaker === "me") return { kind: "me" };
  if (speaker === "them") return { kind: "them" };
  return null;
}

/** Plain (non-localized) display label, matching the Rust exporters' "Me"/
 * "Them" convention. For UI text that goes through i18n (svelte-i18n's `$t`),
 * branch on `resolveSpeakerLabel`'s `kind` instead so "me"/"them" get
 * translated. */
export function speakerPlainLabel(
  speaker: Speaker | null | undefined,
): string | null {
  const label = resolveSpeakerLabel(speaker);
  if (!label) return null;
  return label.kind === "me" ? "Me" : "Them";
}
