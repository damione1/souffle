import { Channel } from "@tauri-apps/api/core";
import { commands, unwrap } from "./generated";

export async function renameMeeting(id: string, title: string): Promise<void> {
  await unwrap(commands.renameMeeting(id, title));
}

export async function saveMeetingNotes(id: string, notes: string | null): Promise<void> {
  await unwrap(commands.saveMeetingNotes(id, notes));
}
import type {
  ExportFormat,
  MeetingAudioSession,
  MeetingCalendarContext,
  MeetingListItem,
  MeetingTranscript,
  SearchResult,
  SummarizeProgress,
  TranscriptionSegment,
} from "../types";

export async function listMeetings(): Promise<MeetingListItem[]> {
  return unwrap(commands.listMeetings());
}

export async function getMeeting(id: string): Promise<MeetingTranscript> {
  return unwrap(commands.getMeeting(id));
}

/** Recorded audio files for a meeting (empty if recording was off, or
 * nothing survived retention). */
export async function getMeetingAudio(meetingId: string): Promise<MeetingAudioSession[]> {
  return unwrap(commands.getMeetingAudio(meetingId));
}

export async function startMeetingRecording(
  title: string,
  calendar: MeetingCalendarContext | null,
  onSegment: (segment: TranscriptionSegment) => void,
): Promise<void> {
  const channel = new Channel<TranscriptionSegment>();
  channel.onmessage = onSegment;
  await unwrap(commands.startMeetingRecording(title, calendar, channel));
}

export async function resumeMeetingRecording(
  id: string,
  onSegment: (segment: TranscriptionSegment) => void,
): Promise<void> {
  const channel = new Channel<TranscriptionSegment>();
  channel.onmessage = onSegment;
  await unwrap(commands.resumeMeetingRecording(id, channel));
}

export async function stopMeetingRecording(): Promise<string> {
  return unwrap(commands.stopMeetingRecording());
}

/** The meeting id paused by the system-sleep handler, if any. Non-destructive:
 * called on wake (possibly more than once, while a sleep-triggered stop is
 * still draining) to decide whether to offer/auto-start a resume. */
export async function peekSleepPausedMeeting(): Promise<string | null> {
  return commands.peekSleepPausedMeeting();
}

/** Clear the meeting id paused by the system-sleep handler. Call after a
 * resume actually starts, or after the user explicitly declines to resume. */
export async function clearSleepPausedMeeting(): Promise<void> {
  return commands.clearSleepPausedMeeting();
}

/** `templateId` picks the summary template for the final pass; null lets
 * the backend fall back to the default template from settings. */
export async function summarizeMeeting(
  id: string,
  model: string,
  templateId: string | null,
  onProgress: (progress: SummarizeProgress) => void,
): Promise<void> {
  const channel = new Channel<SummarizeProgress>();
  channel.onmessage = onProgress;
  await unwrap(commands.summarizeMeeting(id, model, templateId, channel));
}

export async function deleteMeeting(id: string): Promise<void> {
  await unwrap(commands.deleteMeeting(id));
}

export async function searchText(query: string, limit?: number): Promise<SearchResult[]> {
  return unwrap(commands.searchText(query, limit ?? null));
}

export async function saveEditedTranscript(id: string, editedTranscript: string | null): Promise<void> {
  await unwrap(commands.saveEditedTranscript(id, editedTranscript));
}

export async function applyLiveParagraphEdit(
  meetingId: string,
  segmentStart: number,
  segmentEnd: number,
  newText: string,
): Promise<void> {
  await unwrap(commands.applyLiveParagraphEdit(meetingId, segmentStart, segmentEnd, newText));
}

/** Suggested filename for a meeting export (e.g. "2026-07-09-weekly-sync.md"). */
export async function exportMeetingFilename(id: string, format: ExportFormat): Promise<string> {
  return unwrap(commands.exportMeetingFilename(id, format));
}

/** Render a meeting export without writing to disk. */
export async function exportMeetingPreview(id: string, format: ExportFormat): Promise<string> {
  return unwrap(commands.exportMeetingPreview(id, format));
}

/** Render a meeting export and write it to `path`. */
export async function exportMeetingToFile(id: string, format: ExportFormat, path: string): Promise<void> {
  await unwrap(commands.exportMeetingToFile(id, format, path));
}

/** Suggested filename for a meeting audio export (e.g. "2026-07-09-weekly-sync.ogg"). */
export async function exportMeetingAudioFilename(id: string): Promise<string> {
  return unwrap(commands.exportMeetingAudioFilename(id));
}

/** Copy recorded audio for a meeting to `path` (or `{stem}-N.ogg` next to it). */
export async function exportMeetingAudioToFile(id: string, path: string): Promise<void> {
  await unwrap(commands.exportMeetingAudioToFile(id, path));
}

/** Native save dialog + write markdown/subtitles. Cancel is a no-op. */
export async function saveMeetingExport(id: string, format: ExportFormat): Promise<void> {
  await unwrap(commands.saveMeetingExport(id, format));
}

/** Native save dialog + copy recorded audio. Cancel is a no-op. */
export async function saveMeetingAudioExport(id: string): Promise<void> {
  await unwrap(commands.saveMeetingAudioExport(id));
}
