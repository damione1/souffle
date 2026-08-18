import { commands, unwrap } from "./generated";
import type { DictationPolishResult } from "../types";

export async function polishDictation(
  text: string,
  focusedApp?: string | null,
  rewriteOf?: string | null,
): Promise<DictationPolishResult> {
  return unwrap(commands.polishDictation(text, focusedApp ?? null, rewriteOf ?? null));
}
