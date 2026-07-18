import { commands, unwrap } from "./generated";
import type { MeetingSpeaker, MeetingTranscript, RetagMeetingSpeakerRequest, SpeakerProfile } from "../types";

export async function renameSpeaker(id: number, name: string): Promise<void> {
  await unwrap(commands.renameSpeaker(id, name));
}

export async function listSpeakers(): Promise<MeetingSpeaker[]> {
  return unwrap(commands.listSpeakers());
}

export async function listSpeakerProfiles(): Promise<SpeakerProfile[]> {
  return unwrap(commands.listSpeakerProfiles());
}

export async function deleteSpeaker(id: number): Promise<void> {
  await unwrap(commands.deleteSpeaker(id));
}

export async function mergeSpeakers(sourceId: number, targetId: number): Promise<void> {
  await unwrap(commands.mergeSpeakers(sourceId, targetId));
}

export async function setSpeakerIsMe(id: number, isMe: boolean): Promise<void> {
  await unwrap(commands.setSpeakerIsMe(id, isMe));
}

export async function retagMeetingSpeaker(
  request: RetagMeetingSpeakerRequest,
): Promise<MeetingTranscript> {
  return unwrap(commands.retagMeetingSpeaker(request));
}
