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

/** Human-readable byte size, e.g. "482 KB" or "1.3 GB". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unitIndex = 0;
  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }
  const precision = value < 10 ? 1 : 0;
  return `${value.toFixed(precision)} ${units[unitIndex]}`;
}
