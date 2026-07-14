export type TranscriptWordToken =
  | { kind: "word"; text: string }
  | { kind: "gap"; text: string };

const WORD_PATTERN = /[\p{L}\p{M}][\p{L}\p{M}\p{N}'-]*|[\p{L}\p{M}\p{N}'-]+/gu;

/** Split transcript text into clickable words and non-word gaps (spaces, punctuation). */
export function tokenizeTranscriptWords(text: string): TranscriptWordToken[] {
  if (!text) return [];

  const tokens: TranscriptWordToken[] = [];
  let lastIndex = 0;

  for (const match of text.matchAll(WORD_PATTERN)) {
    const start = match.index ?? 0;
    if (start > lastIndex) {
      tokens.push({ kind: "gap", text: text.slice(lastIndex, start) });
    }
    tokens.push({ kind: "word", text: match[0] });
    lastIndex = start + match[0].length;
  }

  if (lastIndex < text.length) {
    tokens.push({ kind: "gap", text: text.slice(lastIndex) });
  }

  return tokens;
}

/** Whether a token should open the dictionary-alias popover. */
export function isClickableTranscriptWord(word: string): boolean {
  if (word.length < 2) return false;
  return /\p{L}/u.test(word);
}
