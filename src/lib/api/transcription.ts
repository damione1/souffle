import { Channel } from "@tauri-apps/api/core";
import { commands, unwrap } from "./generated";
import type {
  AppStateMachine,
  DictationEntry,
  DownloadProgress,
  PasteMethod,
  PillHoldKind,
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

/** Whether the offline speaker-recognition (diarization) models are on disk. */
export async function getDiarizeModelsStatus(): Promise<boolean> {
  return commands.getDiarizeModelsStatus();
}

/** Download the offline speaker-recognition models (~32MB), streaming
 * progress like {@link downloadModel}. */
export async function downloadDiarizeModels(
  onProgress: (progress: DownloadProgress) => void,
): Promise<void> {
  const channel = new Channel<DownloadProgress>();
  channel.onmessage = onProgress;
  await unwrap(commands.downloadDiarizeModels(channel));
}

export async function deleteModel(selection: TranscriptionProfileSelection): Promise<void> {
  await unwrap(commands.deleteModel(selection));
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

export async function pasteText(
  text: string,
  delayMs: number,
  method: PasteMethod = "clipboard",
): Promise<void> {
  await unwrap(commands.pasteText(text, delayMs, method));
}

export async function getMachineState(): Promise<AppStateMachine> {
  return unwrap(commands.getMachineState());
}

/** Keep the pill window visible past the current recording state (e.g. while
 * dictation polish reformulates in the background after transcription stops). */
export async function pillHold(kind: PillHoldKind): Promise<void> {
  await unwrap(commands.pillHold(kind));
}

/** Release a hold set by {@link pillHold}. Safe to call with nothing held. */
export async function pillRelease(): Promise<void> {
  await unwrap(commands.pillRelease());
}

/** Resize the pill window, keeping its top edge pinned below the menu bar
 * and staying horizontally centered. */
export async function pillResize(width: number, height: number): Promise<void> {
  await unwrap(commands.pillResize(width, height));
}

export async function recoverState(): Promise<AppStateMachine> {
  return unwrap(commands.recoverState());
}
