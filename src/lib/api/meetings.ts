import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  MeetingListItem,
  MeetingTranscript,
  SummarizeProgress,
  TranscriptionSegment,
} from "../types";

export async function listMeetings(): Promise<MeetingListItem[]> {
  return invoke<MeetingListItem[]>("list_meetings");
}

export async function getMeeting(id: string): Promise<MeetingTranscript> {
  return invoke<MeetingTranscript>("get_meeting", { id });
}

export async function startMeetingRecording(
  title: string,
  onSegment: (segment: TranscriptionSegment) => void,
): Promise<void> {
  const channel = new Channel<TranscriptionSegment>();
  channel.onmessage = onSegment;
  await invoke("start_meeting_recording", { title, channel });
}

export async function stopMeetingRecording(): Promise<string> {
  return invoke<string>("stop_meeting_recording");
}

export async function summarizeMeeting(
  id: string,
  model: string,
  onProgress: (progress: SummarizeProgress) => void,
): Promise<void> {
  const channel = new Channel<SummarizeProgress>();
  channel.onmessage = onProgress;
  await invoke("summarize_meeting", { id, model, channel });
}

export async function deleteMeeting(id: string): Promise<void> {
  await invoke("delete_meeting", { id });
}
