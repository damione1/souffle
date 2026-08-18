import { describe, expect, it, vi, afterEach } from "vitest";
import { get } from "svelte/store";
import { t } from "svelte-i18n";
import type { InputRouteNotice } from "../../types";
import { createMicToast, micToastCopy } from "./mic-toast.svelte";

const translate = (key: string, options?: { values?: Record<string, string> }) =>
  get(t)(key, options);

function notice(partial: Partial<InputRouteNotice> & Pick<InputRouteNotice, "reason">): InputRouteNotice {
  return {
    from_name: null,
    to_name: null,
    to_uid: null,
    transport: null,
    ...partial,
  };
}

describe("micToastCopy", () => {
  it("formats a switched route", () => {
    const copy = micToastCopy(
      notice({
        reason: "switched",
        from_name: "Built-in",
        to_name: "USB Webcam Mic",
      }),
      translate,
    );
    expect(copy.title).toBe("Microphone changed");
    expect(copy.detail).toBe("Built-in  →  USB Webcam Mic");
    expect(copy.hint).toContain("listening");
  });

  it("uses the bluetooth hint for headset connect", () => {
    const copy = micToastCopy(
      notice({
        reason: "connected",
        to_name: "AirPods Pro",
        transport: "bluetooth",
      }),
      translate,
    );
    expect(copy.title).toBe("Microphone connected");
    expect(copy.detail).toBe("AirPods Pro");
    expect(copy.hint).toContain("Bluetooth");
  });

  it("uses lost_none when the previous name is missing", () => {
    const copy = micToastCopy(notice({ reason: "lost" }), translate);
    expect(copy.detail).toBe("Connect a microphone in Settings");
  });
});

describe("createMicToast", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("replaces the current notice and auto-hides", () => {
    vi.useFakeTimers();
    const toast = createMicToast(5_000);
    toast.show(notice({ reason: "connected", to_name: "USB" }));
    expect(toast.current?.to_name).toBe("USB");
    toast.show(notice({ reason: "switched", to_name: "Built-in" }));
    expect(toast.current?.reason).toBe("switched");
    vi.advanceTimersByTime(5_000);
    expect(toast.current).toBeNull();
  });

  it("queues connected behind a switched notice", () => {
    vi.useFakeTimers();
    const toast = createMicToast(5_000);
    toast.show(notice({ reason: "switched", from_name: "Built-in", to_name: "USB" }));
    toast.show(notice({ reason: "connected", to_name: "AirPods", transport: "bluetooth" }));
    expect(toast.current?.reason).toBe("switched");
    expect(toast.current?.to_name).toBe("USB");
    vi.advanceTimersByTime(5_000);
    expect(toast.current?.reason).toBe("connected");
    expect(toast.current?.to_name).toBe("AirPods");
    vi.advanceTimersByTime(5_000);
    expect(toast.current).toBeNull();
  });

  it("ignores a duplicate lost notice", () => {
    vi.useFakeTimers();
    const toast = createMicToast(5_000);
    toast.show(notice({ reason: "lost", from_name: "USB" }));
    toast.show(notice({ reason: "lost", from_name: "USB" }));
    expect(toast.current?.from_name).toBe("USB");
    vi.advanceTimersByTime(5_000);
    expect(toast.current).toBeNull();
  });

  it("dismiss clears current and pending", () => {
    vi.useFakeTimers();
    const toast = createMicToast(5_000);
    toast.show(notice({ reason: "switched", to_name: "USB" }));
    toast.show(notice({ reason: "connected", to_name: "AirPods" }));
    toast.dismiss();
    expect(toast.current).toBeNull();
    vi.advanceTimersByTime(5_000);
    expect(toast.current).toBeNull();
  });
});
