import { commands, unwrap } from "./generated";
import type { DiagnosticsBundle, LogLevel, UpdateCheckResult } from "../types";

export async function getLogTail(maxLines = 80): Promise<string> {
  return unwrap(commands.getLogTail(maxLines));
}

export async function getDiagnosticsBundle(): Promise<DiagnosticsBundle> {
  return unwrap(commands.getDiagnosticsBundle());
}

export async function getDiagnosticsText(): Promise<string> {
  return unwrap(commands.getDiagnosticsText());
}

export async function checkForUpdates(): Promise<UpdateCheckResult> {
  return unwrap(commands.checkForUpdates());
}

export async function getReleaseNotesForVersion(
  version: string,
): Promise<string | null> {
  const notes = await unwrap(commands.getReleaseNotesForVersion(version));
  const trimmed = notes?.trim();
  return trimmed ? trimmed : null;
}

export async function getAppVersion(): Promise<string> {
  return commands.getAppVersion();
}

export type { LogLevel };
