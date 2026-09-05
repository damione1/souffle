import { describe, expect, it } from "vitest";
import {
  buildMicrophoneList,
  keepConnectedDevices,
  removeKnownDevice,
  reorderMicrophoneList,
} from "./microphone-list";
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

describe("removeKnownDevice", () => {
  const priority: InputPriority = {
    priorities: ["usb", "ghost", "builtin"],
    hidden: ["ghost"],
    known: [
      { uid: "ghost", name: "Old headset", last_seen: 1 },
      { uid: "builtin", name: "Built-in", last_seen: 2 },
    ],
  };

  it("clears the uid from priorities, hidden, and known", () => {
    const next = removeKnownDevice(priority, "ghost");
    expect(next.priorities).toEqual(["usb", "builtin"]);
    expect(next.hidden).toEqual([]);
    expect(next.known.map((entry) => entry.uid)).toEqual(["builtin"]);
  });

  it("preserves the relative order of what survives", () => {
    const next = removeKnownDevice(priority, "usb");
    expect(next.priorities).toEqual(["ghost", "builtin"]);
  });

  it("is a no-op when the uid is absent from all three lists", () => {
    const next = removeKnownDevice(priority, "unknown-uid");
    expect(next).toEqual(priority);
  });
});

describe("keepConnectedDevices", () => {
  const priority: InputPriority = {
    priorities: ["usb", "ghost", "builtin"],
    hidden: ["ghost", "builtin"],
    known: [
      { uid: "ghost", name: "Old headset", last_seen: 1 },
      { uid: "usb", name: "USB Mic", last_seen: 2 },
      { uid: "builtin", name: "Built-in", last_seen: 3 },
    ],
  };

  it("keeps connected devices and drops the rest, preserving order", () => {
    const next = keepConnectedDevices(priority, ["builtin", "usb"]);
    expect(next.priorities).toEqual(["usb", "builtin"]);
    expect(next.hidden).toEqual(["builtin"]);
    expect(next.known.map((entry) => entry.uid)).toEqual(["usb", "builtin"]);
  });

  it("is a no-op when every known device is connected", () => {
    const next = keepConnectedDevices(priority, ["usb", "ghost", "builtin"]);
    expect(next).toEqual(priority);
  });

  it("drops everything when nothing is connected", () => {
    const next = keepConnectedDevices(priority, []);
    expect(next).toEqual({ priorities: [], hidden: [], known: [] });
  });
});
