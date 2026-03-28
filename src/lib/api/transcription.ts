import { Channel } from "@tauri-apps/api/core";
import { commands, unwrap } from "./generated";
import type {
  AppStateMachine,
  DictationEntry,
  DownloadProgress,
  TranscriptionCatalog,
  TranscriptionProfileSelection,
  TranscriptionRuntimeStatus,
  TranscriptionSegment,
} from "../types";

export async function getTranscriptionCatalog(): Promise<TranscriptionCatalog> {
  return unwrap(commands.getTranscriptionCatalog());
}

export async function getModelStatus(
  selection: TranscriptionProfileSelection,
): Promise<TranscriptionRuntimeStatus> {
  return unwrap(commands.getModelStatus(selection));
}

export async function downloadModel(
  selection: TranscriptionProfileSelection,
  onProgress: (progress: DownloadProgress) => void,
): Promise<void> {
  const channel = new Channel<DownloadProgress>();
  channel.onmessage = onProgress;
  await unwrap(commands.downloadModel(selection, channel));
}

export async function loadModel(selection: TranscriptionProfileSelection): Promise<void> {
  await unwrap(commands.loadModel(selection));
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

export async function getMachineState(): Promise<AppStateMachine> {
  return unwrap(commands.getMachineState());
}

export async function recoverState(): Promise<AppStateMachine> {
  return unwrap(commands.recoverState());
}
