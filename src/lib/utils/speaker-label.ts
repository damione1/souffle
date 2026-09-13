import type { Speaker } from "../types";

/** Plain, non-localized label. Mirrors `Speaker::display_name` in Rust, which
 * is what every exporter writes, so copied transcript text matches an exported
 * one. */
const SPEAKER_PLAIN_LABEL: Record<Speaker, string> = {
  me: "Me",
  them: "Them",
};

/** i18n key for the speaker badge. */
const SPEAKER_I18N_KEY: Record<Speaker, string> = {
  me: "transcript.me",
  them: "transcript.them",
};

/** Text colour class for the speaker badge. */
const SPEAKER_TEXT_CLASS: Record<Speaker, string> = {
  me: "text-accent",
  them: "text-secondary",
};

/** The contract's variants as runtime values. specta generates types, not
 * values, so this is the one place the union is enumerated at runtime. */
export const SPEAKERS = Object.keys(SPEAKER_PLAIN_LABEL) as readonly Speaker[];

/** Narrowing guard. `segments.speaker` is free `TEXT` in SQLite and still
 * holds `spk:<id>` labels from the dropped persistent-speaker feature; the
 * backend already turns those into `null` (`Speaker::parse`), this is the
 * second line of defence for anything read outside that path. */
export function isSpeaker(value: unknown): value is Speaker {
  return typeof value === "string" && Object.hasOwn(SPEAKER_PLAIN_LABEL, value);
}

/** `null`, `undefined` and anything outside the contract resolve to `null`:
 * callers just do not render a badge. */
export function resolveSpeaker(
  value: Speaker | string | null | undefined,
): Speaker | null {
  return isSpeaker(value) ? value : null;
}

export function speakerI18nKey(speaker: Speaker): string {
  return SPEAKER_I18N_KEY[speaker];
}

export function speakerTextClass(speaker: Speaker): string {
  return SPEAKER_TEXT_CLASS[speaker];
}

/** Plain (non-localized) display label. For UI text that goes through i18n,
 * use `speakerI18nKey` instead so "me"/"them" get translated. */
export function speakerPlainLabel(
  speaker: Speaker | string | null | undefined,
): string | null {
  const resolved = resolveSpeaker(speaker);
  return resolved === null ? null : SPEAKER_PLAIN_LABEL[resolved];
}
