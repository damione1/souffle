import { commands, unwrap } from "./generated";
import type { OllamaStatus } from "../types";

export async function getOllamaStatus(): Promise<OllamaStatus> {
  return unwrap(commands.checkOllama());
}
