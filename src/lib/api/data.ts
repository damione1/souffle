import { commands, unwrap } from "./generated";
import type { DataStats, McpSetupInfo } from "../types";

/** Database size on disk plus meeting/dictation counts. */
export async function getDataStats(): Promise<DataStats> {
  return unwrap(commands.getDataStats());
}

/** Start a full data archive export into a fresh folder under `destDir`.
 * Resolves once the destination is validated and the background export has
 * started; progress arrives via the `archive-export-progress` event. */
export async function exportArchive(destDir: string): Promise<void> {
  await unwrap(commands.exportArchive(destDir));
}

/** Reveal the app's data directory (database, logs, models) in Finder. */
export async function revealDataDir(): Promise<void> {
  await unwrap(commands.revealDataDir());
}

/** Sidecar path plus copy/paste snippets for MCP client setup. */
export async function getMcpSetupInfo(): Promise<McpSetupInfo> {
  return unwrap(commands.getMcpSetupInfo());
}

/** Spawn the sidecar and verify it speaks MCP over stdio. */
export async function testMcpConnection(): Promise<string> {
  return unwrap(commands.testMcpConnection());
}
