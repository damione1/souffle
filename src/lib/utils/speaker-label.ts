import type { MeetingSpeaker, Speaker } from "../types";

/** Parsed result of a speaker label: the fixed "me"/"them" cases, a
 * persistent speaker resolved to its display name, or a persistent speaker
 * whose id isn't (or is no longer) in the meeting's `speakers` list. */
export type SpeakerLabel =
  | { kind: "me" }
  | { kind: "them" }
  | { kind: "named"; name: string }
  | { kind: "unknown"; id: number };

const PERSISTENT_ID = /^spk:(\d+)$/;

/** Resolve a segment/paragraph's `speaker` value against a meeting's
 * `speakers` list. `null`/`undefined` (no diarization) and a malformed
 * label both resolve to `null` — callers should just not render a badge. */
export function resolveSpeakerLabel(
  speaker: Speaker | null | undefined,
  speakers: MeetingSpeaker[],
): SpeakerLabel | null {
  if (speaker == null) return null;
  if (speaker === "me") return { kind: "me" };
  if (speaker === "them") return { kind: "them" };

  const match = PERSISTENT_ID.exec(speaker);
  if (!match) return null;
  const id = Number(match[1]);
  const found = speakers.find((s) => s.id === id);
  return found ? { kind: "named", name: found.name } : { kind: "unknown", id };
}

/** Plain (non-localized) display label, matching the Rust exporters' "Me"/
 * "Them"/name/"Speaker <id>" convention. For UI text that goes through
 * i18n (svelte-i18n's `$t`), branch on `resolveSpeakerLabel`'s `kind`
 * instead so "me"/"them" get translated. */
export function speakerPlainLabel(
  speaker: Speaker | null | undefined,
  speakers: MeetingSpeaker[],
): string | null {
  const label = resolveSpeakerLabel(speaker, speakers);
  if (!label) return null;
  switch (label.kind) {
    case "me":
      return "Me";
    case "them":
      return "Them";
    case "named":
      return label.name;
    case "unknown":
      return `Speaker ${label.id}`;
  }
}
