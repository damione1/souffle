import { Channel, invoke } from "@tauri-apps/api/core";
import type {
  DictationEntry,
  DownloadProgress,
  ModelStatus,
  TranscriptionCatalog,
  TranscriptionSegment,
} from "../types";

export async function getTranscriptionCatalog(): Promise<TranscriptionCatalog> {
  return invoke<TranscriptionCatalog>("get_transcription_catalog");
}

export async function getModelStatus(): Promise<ModelStatus> {
  return invoke<ModelStatus>("get_model_status");
}

export async function downloadModel(
  onProgress: (progress: DownloadProgress) => void,
): Promise<void> {
  const channel = new Channel<DownloadProgress>();
  channel.onmessage = onProgress;
  await invoke("download_model", { channel });
}

export async function loadModel(): Promise<void> {
  await invoke("load_model");
}

export async function startStreamingTranscription(
  onSegment: (segment: TranscriptionSegment) => void,
): Promise<void> {
  const channel = new Channel<TranscriptionSegment>();
  channel.onmessage = onSegment;
  await invoke("start_transcription", { channel });
}

export async function stopStreamingTranscription(): Promise<void> {
  await invoke("stop_transcription");
}

export async function listDictationEntries(limit = 50): Promise<DictationEntry[]> {
  return invoke<DictationEntry[]>("list_dictation_entries", { limit });
}

export async function addDictationEntry(text: string): Promise<void> {
  await invoke("add_dictation_entry", { text });
}

export async function deleteDictationEntry(id: string): Promise<void> {
  await invoke("delete_dictation_entry", { id });
}

export async function clearDictationHistory(): Promise<void> {
  await invoke("clear_dictation_history");
}

export async function pasteText(text: string, delayMs: number): Promise<void> {
  await invoke("paste_text", { text, delayMs });
}
