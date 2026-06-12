import { Channel } from "@tauri-apps/api/core";
import { commands, unwrap } from "./generated";

export async function renameMeeting(id: string, title: string): Promise<void> {
  await unwrap(commands.renameMeeting(id, title));
}
import type {
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

export async function startMeetingRecording(
  title: string,
  onSegment: (segment: TranscriptionSegment) => void,
): Promise<void> {
  const channel = new Channel<TranscriptionSegment>();
  channel.onmessage = onSegment;
  await unwrap(commands.startMeetingRecording(title, channel));
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

export async function summarizeMeeting(
  id: string,
  model: string,
  onProgress: (progress: SummarizeProgress) => void,
): Promise<void> {
  const channel = new Channel<SummarizeProgress>();
  channel.onmessage = onProgress;
  await unwrap(commands.summarizeMeeting(id, model, channel));
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
