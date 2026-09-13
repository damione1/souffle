import { commands, events, unwrap } from "./generated";
import type {
  InstallBlockReason,
  UpdateDownloadStatus,
  UpdatePhase,
} from "../types";

export type { InstallBlockReason, UpdateDownloadStatus, UpdatePhase };

export async function getUpdateDownloadStatus(): Promise<UpdateDownloadStatus> {
  return unwrap(commands.getUpdateDownloadStatus());
}

export async function getUpdateInstallBlock(): Promise<InstallBlockReason | null> {
  return unwrap(commands.getUpdateInstallBlock());
}

export async function downloadUpdate(): Promise<UpdateDownloadStatus> {
  return unwrap(commands.downloadUpdate());
}

export async function cancelUpdateDownload(): Promise<UpdateDownloadStatus> {
  return unwrap(commands.cancelUpdateDownload());
}

export async function installUpdate(): Promise<void> {
  await unwrap(commands.installUpdate());
}

export function listenUpdateDownloadProgress(
  handler: (status: UpdateDownloadStatus) => void,
): Promise<() => void> {
  return events.updateDownloadProgress.listen((event) => {
    handler(event.payload);
  });
}
