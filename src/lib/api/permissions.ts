import { commands, unwrap } from "./generated";
import type { PermissionKind, PermissionStatus, PermState } from "../types";

export type { PermissionKind };

export async function getPermissionStatus(): Promise<PermissionStatus> {
  return unwrap(commands.getPermissionStatus());
}

export async function requestPermission(kind: PermissionKind): Promise<PermState> {
  return unwrap(commands.requestPermission(kind));
}
