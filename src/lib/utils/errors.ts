/** Extract a human-readable message from an unknown error */
export function errorMessage(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}
