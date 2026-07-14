import type { AudioInputDevice, InputPriority, TransportType } from "../../types";

export type MicrophoneListEntry = {
  uid: string;
  name: string;
  transport: TransportType;
  isDefault: boolean;
  connected: boolean;
  hidden: boolean;
  lastSeen: number | null;
};

/** Merge connected devices, remembered devices, and priority order for Settings. */
export function buildMicrophoneList(
  connected: AudioInputDevice[],
  priority: InputPriority,
): MicrophoneListEntry[] {
  const connectedByUid = new Map(connected.map((device) => [device.uid, device]));
  const hidden = new Set(priority.hidden);
  const knownByUid = new Map(priority.known.map((entry) => [entry.uid, entry]));
  const seen = new Set<string>();
  const ordered: MicrophoneListEntry[] = [];

  const push = (uid: string) => {
    if (seen.has(uid)) return;
    seen.add(uid);
    const live = connectedByUid.get(uid);
    const known = knownByUid.get(uid);
    ordered.push({
      uid,
      name: live?.name ?? known?.name ?? uid,
      transport: live?.transport ?? "unknown",
      isDefault: live?.is_default ?? false,
      connected: live !== undefined,
      hidden: hidden.has(uid),
      lastSeen: live ? null : known?.last_seen ?? null,
    });
  };

  for (const uid of priority.priorities) {
    push(uid);
  }
  for (const entry of priority.known) {
    push(entry.uid);
  }
  for (const device of connected) {
    push(device.uid);
  }

  return ordered;
}

export function reorderMicrophoneList(
  list: MicrophoneListEntry[],
  uid: string,
  direction: -1 | 1,
): string[] | null {
  const index = list.findIndex((entry) => entry.uid === uid);
  const target = index + direction;
  if (index < 0 || target < 0 || target >= list.length) {
    return null;
  }
  const uids = list.map((entry) => entry.uid);
  [uids[index], uids[target]] = [uids[target], uids[index]];
  return uids;
}

export function transportLabelKey(transport: TransportType): string {
  switch (transport) {
    case "built_in":
      return "settings_audio.transport_built_in";
    case "usb":
      return "settings_audio.transport_usb";
    case "bluetooth":
    case "bluetooth_le":
      return "settings_audio.transport_bluetooth";
    case "virtual":
      return "settings_audio.transport_virtual";
    case "aggregate":
      return "settings_audio.transport_aggregate";
    default:
      return "settings_audio.transport_unknown";
  }
}
