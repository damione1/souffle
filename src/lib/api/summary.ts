import { Channel } from "@tauri-apps/api/core";
import { commands, unwrap } from "./generated";
import type { OllamaPullProgress, SummaryProvidersStatus } from "../types";

export const RECOMMENDED_OLLAMA_MODEL = "qwen2.5:7b";

export async function getSummaryProvidersStatus(): Promise<SummaryProvidersStatus> {
  return unwrap(commands.checkSummaryProviders());
}

export async function pullRecommendedOllamaModel(
  onProgress: (progress: OllamaPullProgress) => void,
): Promise<string> {
  const channel = new Channel<OllamaPullProgress>();
  channel.onmessage = onProgress;
  return unwrap(commands.pullRecommendedOllamaModel(channel));
}
