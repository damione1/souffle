import { commands, unwrap } from "./generated";
import type { DictationPolishResult } from "../types";

export async function polishDictation(text: string): Promise<DictationPolishResult> {
  return unwrap(commands.polishDictation(text));
}
