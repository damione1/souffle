import { commands, unwrap } from "./generated";
import type { PermissionStatus, PermState } from "../types";

export async function getPermissionStatus(): Promise<PermissionStatus> {
  return unwrap(commands.getPermissionStatus());
}

export type PermissionKind = "microphone" | "system_audio" | "accessibility";

export async function requestPermission(kind: PermissionKind): Promise<PermState> {
  return unwrap(commands.requestPermission(kind));
}
