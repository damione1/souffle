import { listSnippets } from "../../api/snippets";
import { getAppState } from "../../stores/app.svelte";
import type { SnippetEntry } from "../../types";

/** Case- and accent-insensitive form used to compare a trigger with the
 * transcript. Mirrors `fold_trigger` in src-tauri/src/db/snippets.rs. */
export function foldSnippetTrigger(text: string): string {
  return text.normalize("NFD").replace(/\p{M}/gu, "").toLowerCase();
}

/** Offset in `text` just past the prefix whose folded form is `foldedLength`
 * code units long. Folding is done per code point so the raw and folded
 * strings stay aligned even when the input is decomposed (NFD), where a
 * `snippet.trigger.length` slice would land mid-character. Combining marks
 * that trail the last base character belong to the prefix too. */
function rawPrefixEnd(text: string, foldedLength: number): number {
  let folded = 0;
  let offset = 0;
  for (const ch of text) {
    if (folded >= foldedLength && !/^\p{M}$/u.test(ch)) break;
    folded += foldSnippetTrigger(ch).length;
    offset += ch.length;
  }
  return offset;
}

/** Punctuation the engine glues to the trigger ("signature mail, et …"):
 * kept attached to the expansion rather than separated by a space. */
const LEADING_PUNCTUATION = /^[.,!?:;]/;

/** Replace a spoken trigger at the start of `text` by its expansion.
 *
 * Longest trigger wins, the trigger must end on a word boundary, and the
 * rest of the transcript is kept after the expansion. Returns `null` when
 * no snippet matches so the caller falls through to today's path. */
export function applySnippet(text: string, snippets: readonly SnippetEntry[]): string | null {
  if (snippets.length === 0) return null;

  const folded = foldSnippetTrigger(text);
  const candidates = snippets
    .map((snippet) => ({ snippet, key: foldSnippetTrigger(snippet.trigger.trim()) }))
    .filter(({ key }) => key.length > 0)
    .sort((a, b) => b.key.length - a.key.length);

  for (const { snippet, key } of candidates) {
    if (!folded.startsWith(key)) continue;
    const next = folded[key.length];
    if (next !== undefined && /[\p{L}\p{N}]/u.test(next)) continue;

    const remainder = text.slice(rawPrefixEnd(text, key.length)).trimStart();
    if (!remainder) return snippet.expansion;
    if (LEADING_PUNCTUATION.test(remainder)) return snippet.expansion + remainder;
    return `${snippet.expansion} ${remainder}`;
  }
  return null;
}

/** Reload the snippet list into the app store. Failure keeps the previous
 * list: a missing store must not block a dictation from finishing. */
export async function refreshSnippets(): Promise<void> {
  try {
    getAppState().snippets = await listSnippets();
  } catch (e) {
    console.warn("Failed to load snippets:", e);
  }
}
