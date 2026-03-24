import { Channel } from "@tauri-apps/api/core";
import { commands, unwrap } from "./generated";
import type {
  DictationEntry,
  DownloadProgress,
  TranscriptionCatalog,
  TranscriptionRuntimeStatus,
  TranscriptionSegment,
} from "../types";

export async function getTranscriptionCatalog(): Promise<TranscriptionCatalog> {
  return unwrap(commands.getTranscriptionCatalog());
}

export async function getModelStatus(): Promise<TranscriptionRuntimeStatus> {
  return unwrap(commands.getModelStatus());
}

export async function downloadModel(
  onProgress: (progress: DownloadProgress) => void,
): Promise<void> {
  const channel = new Channel<DownloadProgress>();
  channel.onmessage = onProgress;
  await unwrap(commands.downloadModel(channel));
}

export async function loadModel(): Promise<void> {
  await unwrap(commands.loadModel());
}

export async function startStreamingTranscription(
  onSegment: (segment: TranscriptionSegment) => void,
): Promise<void> {
  const channel = new Channel<TranscriptionSegment>();
  channel.onmessage = onSegment;
  await unwrap(commands.startTranscription(channel));
}

export async function stopStreamingTranscription(): Promise<void> {
  await unwrap(commands.stopTranscription());
}

export async function listDictationEntries(limit = 50): Promise<DictationEntry[]> {
  return unwrap(commands.listDictationEntries(limit));
}

export async function addDictationEntry(text: string): Promise<void> {
  await unwrap(commands.addDictationEntry(text));
}

export async function deleteDictationEntry(id: string): Promise<void> {
  await unwrap(commands.deleteDictationEntry(id));
}

export async function clearDictationHistory(): Promise<void> {
  await unwrap(commands.clearDictationHistory());
}

export async function pasteText(text: string, delayMs: number): Promise<void> {
  await unwrap(commands.pasteText(text, delayMs));
}
