/** "CommandOrControl+Shift+Space" → "⌘ ⇧ Space" for display. */
export function formatShortcutLabel(shortcut: string): string {
  if (!shortcut) return "";
  return shortcut
    .replace(/CommandOrControl/g, "\u2318")
    .replace(/Shift/g, "\u21E7")
    .replace(/Alt/g, "\u2325")
    .replace(/\+/g, " ");
}

/** Position inside a recording, as "M:SS". Stays minute-based past an hour
 * ("61:01") because it renders in fixed-width columns next to the scrubber,
 * where an "H:MM:SS" reading would overflow. Durations use `formatDuration`. */
export function formatTimestamp(seconds: number): string {
  const mins = Math.floor(seconds / 60);
  const secs = Math.floor(seconds % 60);
  return `${mins}:${secs.toString().padStart(2, "0")}`;
}

/** Format ISO date string to locale string */
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleString();
}

/** A span of time, as "M:SS" below an hour and "H:MM:SS" from an hour on.
 * Meetings routinely run past an hour, where a bare "90:00" is hard to read. */
export function formatDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const hours = Math.floor(total / 3600);
  const mins = Math.floor((total % 3600) / 60);
  const secs = `${total % 60}`.padStart(2, "0");
  if (hours === 0) return `${mins}:${secs}`;
  return `${hours}:${`${mins}`.padStart(2, "0")}:${secs}`;
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

/** Relative last-seen age for i18n in Settings microphone list. */
export type LastSeenAge =
  | { kind: "just_now" }
  | { kind: "minutes"; count: number }
  | { kind: "hours"; count: number }
  | { kind: "days"; count: number };

export function lastSeenAge(unixSeconds: number, nowMs = Date.now()): LastSeenAge {
  const deltaSec = Math.max(0, Math.floor(nowMs / 1000) - unixSeconds);
  if (deltaSec < 60) return { kind: "just_now" };
  const minutes = Math.floor(deltaSec / 60);
  if (minutes < 60) return { kind: "minutes", count: minutes };
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return { kind: "hours", count: hours };
  return { kind: "days", count: Math.floor(hours / 24) };
}
