import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/svelte";
import type { AudioInputDevice, InputPriority } from "../../../types";
import MicrophoneSettingsSection from "./MicrophoneSettingsSection.svelte";

const devices: AudioInputDevice[] = [
  { uid: "builtin-mic", name: "Built-in Microphone", transport: "built_in", is_default: true },
];

const priority: InputPriority = {
  priorities: ["builtin-mic"],
  hidden: [],
  known: [{ uid: "builtin-mic", name: "Built-in Microphone", last_seen: 1 }],
};

function renderSection(
  overrides: Partial<{
    sampleRate: number | null;
    sampleRateError: string;
    resettingSampleRate: boolean;
    onResetSampleRate: () => void;
  }> = {},
) {
  const onResetSampleRate = overrides.onResetSampleRate ?? vi.fn();
  render(MicrophoneSettingsSection, {
    props: {
      audioDevices: devices,
      inputPriority: priority,
      selectedDevice: "",
      pinUnavailable: false,
      allowBluetoothMic: false,
      sampleRate: overrides.sampleRate === undefined ? 48_000 : overrides.sampleRate,
      sampleRateError: overrides.sampleRateError ?? "",
      resettingSampleRate: overrides.resettingSampleRate ?? false,
      onDeviceChange: vi.fn(),
      onAllowBluetoothMicChange: vi.fn(),
      onRefreshDevices: vi.fn(),
      onMoveDevice: vi.fn(),
      onToggleHidden: vi.fn(),
      onRemoveDevice: vi.fn(),
      onResetDevices: vi.fn(),
      onResetSampleRate,
    },
  });
  return { onResetSampleRate };
}

describe("MicrophoneSettingsSection sample rate", () => {
  afterEach(cleanup);

  it("shows the current rate without a warning at 48 kHz", () => {
    renderSection({ sampleRate: 48_000 });

    expect(screen.getByTestId("input-sample-rate").textContent).toBe("48 kHz");
    expect(screen.queryByTestId("sample-rate-warning")).toBeNull();
    expect(screen.queryByTestId("reset-sample-rate")).toBeNull();
  });

  it("warns above 48 kHz and does not reset until the button is clicked", async () => {
    const { onResetSampleRate } = renderSection({ sampleRate: 96_000 });

    expect(screen.getByTestId("input-sample-rate").textContent).toBe("96 kHz");
    expect(screen.getByTestId("sample-rate-warning")).toBeTruthy();
    expect(onResetSampleRate).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByTestId("reset-sample-rate"));

    expect(onResetSampleRate).toHaveBeenCalledOnce();
  });

  it("hides the diagnostic when the rate is unknown", () => {
    renderSection({ sampleRate: null });

    expect(screen.queryByTestId("input-sample-rate")).toBeNull();
    expect(screen.queryByTestId("sample-rate-warning")).toBeNull();
    expect(screen.queryByTestId("reset-sample-rate")).toBeNull();
  });
});
