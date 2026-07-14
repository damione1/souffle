import { commands, unwrap } from "./generated";
import type { MeetingSpeaker, MeetingTranscript, RetagMeetingSpeakerRequest } from "../types";

export async function renameSpeaker(id: number, name: string): Promise<void> {
  await unwrap(commands.renameSpeaker(id, name));
}

export async function listSpeakers(): Promise<MeetingSpeaker[]> {
  return unwrap(commands.listSpeakers());
}

export async function retagMeetingSpeaker(
  request: RetagMeetingSpeakerRequest,
): Promise<MeetingTranscript> {
  return unwrap(commands.retagMeetingSpeaker(request));
}
