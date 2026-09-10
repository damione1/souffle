import type { MeetingSystemAudio, SystemAudioReason, SystemAudioStatus } from "../../types";

/** A sentence to show next to the "mic only" label, plus the raw backend detail. */
export type SystemAudioNotice = {
  /** i18n key of the reason, always resolvable. */
  key: string;
  /** Backend detail (a CoreAudio error message), when there is one. */
  detail: string | null;
};

const REASON_KEYS: Record<SystemAudioReason, string> = {
  silent: "meeting_header.system_audio_reason_silent",
  no_samples: "meeting_header.system_audio_reason_no_samples",
  tap_lost: "meeting_header.system_audio_reason_tap_lost",
  start_failed: "meeting_header.system_audio_reason_start_failed",
  probe_failed: "meeting_header.system_audio_reason_probe_failed",
  permission_denied: "meeting_header.system_audio_reason_permission_denied",
  disabled: "meeting_header.system_audio_reason_disabled",
  unsupported: "meeting_header.system_audio_reason_unsupported",
};

const UNKNOWN_KEY = "meeting_header.system_audio_reason_unknown";

function notice(code: SystemAudioReason | null, detail: string | null): SystemAudioNotice {
  return { key: code ? REASON_KEYS[code] : UNKNOWN_KEY, detail };
}

/**
 * What to say about the system-audio leg of the meeting being recorded now,
 * or null while there is nothing to warn about.
 *
 * A live leg that has not carried sound yet is not a warning: at the top of a
 * meeting nobody has spoken, and a false alarm on a healthy session would be
 * worse than the silence this ticket fixes. That only becomes a verdict once
 * the session is over.
 */
export function liveSystemAudioNotice(status: SystemAudioStatus | null): SystemAudioNotice | null {
  if (!status || status.active) return null;
  return notice(status.reason_code, status.reason);
}

/**
 * What to say about a meeting reopened after the fact. Null when the tap
 * ran (both lanes were captured: AC6), and null for meetings recorded
 * before this was tracked. A live-but-silent tap is persisted (AC8) and is
 * not a "mic only" warning: both lanes were captured, they just carried
 * no far-end sound.
 */
export function pastSystemAudioNotice(
  audio: MeetingSystemAudio | null,
): SystemAudioNotice | null {
  if (!audio || audio.active) return null;
  return notice(audio.reason_code, audio.reason);
}

/**
 * A stand-in verdict for the meeting row loaded straight after a stop: it is
 * the header written during the recording, whose verdict only lands with the
 * authoritative save a moment later. Without this the notice would blink out
 * between the stop and `meetingFinalized`. Mirrors the warning path, not the
 * persisted Silent/NoSamples diagnostic.
 */
export function provisionalSystemAudio(
  status: SystemAudioStatus | null,
): MeetingSystemAudio | null {
  if (!status || status.active) return null;
  return {
    active: status.active,
    reason: status.reason,
    reason_code: status.reason_code,
    samples: status.samples,
    signal_samples: status.signal_samples,
  };
}
