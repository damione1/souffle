import type { MeetingAudioSession } from "../../types";

/** Where to play from: which file, and at what offset within it. */
export interface AudioSeekTarget {
  path: string;
  sessionIndex: number;
  offsetSeconds: number;
}

/**
 * Resolve a transcript paragraph's playback target.
 *
 * `recordingSessionIndex` is the paragraph's recording-session position (see
 * `buildMeetingTranscriptBlocks` in `../../utils/paragraphs`) — the same
 * position a recorder used for that session's filename on the backend
 * (`{session_index}.ogg`, listed by `get_meeting_audio`). `startTime` is the
 * paragraph's first segment's `start_time`; the transcription pipeline
 * resets each recording session's clock near zero when it starts (a fresh
 * engine/session), so that value already doubles as a seek offset into that
 * session's audio file — no extra alignment is needed.
 *
 * Returns `null` when there's nothing to seek to: no session was
 * attributed to the paragraph, or that session has no matching audio file
 * (recording was off for it, or it didn't survive retention).
 */
export function resolveAudioSeekTarget(
  recordingSessionIndex: number | null,
  startTime: number,
  audioSessions: MeetingAudioSession[],
): AudioSeekTarget | null {
  if (recordingSessionIndex === null) return null;
  const match = audioSessions.find((session) => session.session_index === recordingSessionIndex);
  if (!match) return null;

  return {
    path: match.path,
    sessionIndex: match.session_index,
    offsetSeconds: Math.max(0, startTime),
  };
}

/** The audio file to show by default, before any paragraph has been
 * clicked: the earliest recorded session, at its start. `null` when there's
 * nothing to play. */
export function defaultAudioTarget(audioSessions: MeetingAudioSession[]): AudioSeekTarget | null {
  if (audioSessions.length === 0) return null;
  const first = audioSessions.reduce((earliest, session) =>
    session.session_index < earliest.session_index ? session : earliest,
  );
  return { path: first.path, sessionIndex: first.session_index, offsetSeconds: 0 };
}

/** What a player should do in response to a seek target: which path to
 * play (only different from `currentPath` when the paragraph belongs to a
 * different recording session) and where to seek once it's loaded. */
export interface PlayCommand {
  path: string;
  seekSeconds: number;
  /** `true` when the player must swap `<audio src>` before it can seek —
   * the caller should wait for `loadedmetadata` before setting
   * `currentTime`, rather than setting it immediately. */
  sessionChanged: boolean;
}

export function buildPlayCommand(target: AudioSeekTarget, currentPath: string | null): PlayCommand {
  return {
    path: target.path,
    seekSeconds: target.offsetSeconds,
    sessionChanged: currentPath !== target.path,
  };
}
