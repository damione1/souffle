import { describe, expect, it } from "vitest";
import { buildMicrophoneList, reorderMicrophoneList } from "./microphone-list";
import type { AudioInputDevice, InputPriority } from "../../types";

const builtin: AudioInputDevice = {
  uid: "builtin",
  name: "Built-in",
  transport: "built_in",
  is_default: true,
};
const usb: AudioInputDevice = {
  uid: "usb",
  name: "USB Mic",
  transport: "usb",
  is_default: false,
};

describe("buildMicrophoneList", () => {
  it("orders by priorities then known then newly connected", () => {
    const priority: InputPriority = {
      priorities: ["usb", "builtin"],
      hidden: ["usb"],
      known: [
        { uid: "ghost", name: "Old headset", last_seen: 1 },
        { uid: "builtin", name: "Built-in", last_seen: 2 },
      ],
    };
    const list = buildMicrophoneList([builtin, usb], priority);
    expect(list.map((entry) => entry.uid)).toEqual(["usb", "builtin", "ghost"]);
    expect(list[0]?.hidden).toBe(true);
    expect(list[2]?.connected).toBe(false);
    expect(list[2]?.lastSeen).toBe(1);
  });
});

describe("reorderMicrophoneList", () => {
  it("swaps adjacent entries", () => {
    const list = buildMicrophoneList([builtin, usb], {
      priorities: [],
      hidden: [],
      known: [],
    });
    expect(reorderMicrophoneList(list, "usb", -1)).toEqual(["usb", "builtin"]);
  });
});
