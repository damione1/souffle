/** Format seconds as "M:SS" */
/** "CommandOrControl+Shift+Space" → "⌘ ⇧ Space" for display. */
export function formatShortcutLabel(shortcut: string): string {
  if (!shortcut) return "";
  return shortcut
    .replace(/CommandOrControl/g, "\u2318")
    .replace(/Shift/g, "\u21E7")
    .replace(/Alt/g, "\u2325")
    .replace(/\+/g, " ");
}

export function formatTimestamp(seconds: number): string {
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

/** Format ISO date string to locale string */
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleString();
}

/** Format duration in seconds as "M:SS" */
export function formatDuration(seconds: number): string {
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}
