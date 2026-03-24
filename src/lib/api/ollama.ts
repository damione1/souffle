import { invoke } from "@tauri-apps/api/core";
import type { OllamaStatus } from "../types";

export async function getOllamaStatus(): Promise<OllamaStatus> {
  return invoke<OllamaStatus>("check_ollama");
}
