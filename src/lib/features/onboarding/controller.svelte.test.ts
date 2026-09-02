import { beforeEach, describe, expect, it, vi } from "vitest";

const { transcriptionApi, settingsApi, startDownload } = vi.hoisted(() => ({
  transcriptionApi: {
    getTranscriptionCatalog: vi.fn(),
  },
  settingsApi: {
    getSettings: vi.fn(),
    saveSettings: vi.fn(),
    getShortcuts: vi.fn(),
    saveShortcuts: vi.fn(),
    listAudioDevices: vi.fn(),
    selectAudioDevice: vi.fn(),
  },
  startDownload: vi.fn(),
}));

vi.mock("../../api/transcription", () => transcriptionApi);
vi.mock("../../api/settings", () => settingsApi);
vi.mock("../transcription/runtime", () => ({
  resetTranscriptionRuntimeState: vi.fn(),
  startTranscriptionModelDownload: startDownload,
}));

import { createOnboardingController } from "./controller.svelte";
import { getAppState } from "../../stores/app.svelte";
import { mockCatalog, mockSettings, mockShortcuts } from "../../test-helpers/fixtures";
import { PERMISSIONS_STORAGE_KEY, SETUP_STORAGE_KEY } from "./setup";

const fakeDevices = [
  { uid: "builtin-mic", name: "Built-in Microphone", transport: "built_in" as const, is_default: true },
  { uid: "usb-mic", name: "External USB Mic", transport: "usb" as const, is_default: false },
];

describe("createOnboardingController", () => {
  const app = getAppState();

  beforeEach(() => {
    localStorage.clear();
    app.showOnboarding = true;
    app.settings = { ...mockSettings };
    app.selectedDevice = "";
    app.transcriptionRuntimePhase = "download_required";
    app.machineState = { state: "idle" };

    transcriptionApi.getTranscriptionCatalog.mockResolvedValue(mockCatalog);
    settingsApi.saveSettings.mockResolvedValue(undefined);
    settingsApi.getShortcuts.mockResolvedValue({ ...mockShortcuts });
    settingsApi.saveShortcuts.mockResolvedValue(undefined);
    settingsApi.listAudioDevices.mockResolvedValue(fakeDevices);
    settingsApi.selectAudioDevice.mockResolvedValue(undefined);
    startDownload.mockResolvedValue(undefined);
  });

  it("starts on permissions and walks to shortcut", async () => {
    const ctrl = createOnboardingController();
    await ctrl.mount();

    expect(ctrl.steps).toEqual(["permissions", "microphone", "model", "shortcut"]);
    expect(ctrl.step).toBe("permissions");

    await ctrl.goNext();
    expect(localStorage.getItem(PERMISSIONS_STORAGE_KEY)).toBeNull();
    expect(ctrl.step).toBe("microphone");

    await ctrl.goNext();
    expect(ctrl.step).toBe("model");
  });

  it("resumes at microphone when permissions were already granted", async () => {
    localStorage.setItem(PERMISSIONS_STORAGE_KEY, "1");
    const ctrl = createOnboardingController();
    await ctrl.mount();
    expect(ctrl.steps).toEqual(["microphone", "model", "shortcut"]);
    expect(ctrl.step).toBe("microphone");
  });

  it("pins a microphone and persists it", async () => {
    localStorage.setItem(PERMISSIONS_STORAGE_KEY, "1");
    const ctrl = createOnboardingController();
    await ctrl.mount();
    expect(ctrl.audioDevices).toEqual(fakeDevices);

    await ctrl.onDeviceChange({
      currentTarget: { value: "usb-mic" },
    } as unknown as Event);

    expect(settingsApi.selectAudioDevice).toHaveBeenCalledWith("usb-mic");
    expect(app.selectedDevice).toBe("usb-mic");
    expect(settingsApi.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ audio_device: "usb-mic" }),
    );
  });

  it("starts the model download from the selected catalog option", async () => {
    const ctrl = createOnboardingController();
    await ctrl.mount();
    expect(ctrl.selectedKey).toBe("kyutai:stt-1b-en_fr");

    await ctrl.beginDownload();

    expect(settingsApi.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({
        transcription_engine_id: "kyutai",
        transcription_model_id: "stt-1b-en_fr",
        transcription_backend_id: "candle",
      }),
    );
    expect(startDownload).toHaveBeenCalled();
  });

  it("records a toggle shortcut", async () => {
    const ctrl = createOnboardingController();
    await ctrl.mount();
    expect(ctrl.toggleShortcut).toBe("CommandOrControl+Shift+S");

    ctrl.startShortcutRecording();
    const event = new KeyboardEvent("keydown", {
      key: " ",
      code: "Space",
      metaKey: true,
      shiftKey: true,
    });
    Object.defineProperty(event, "preventDefault", { value: vi.fn() });
    Object.defineProperty(event, "stopPropagation", { value: vi.fn() });
    ctrl.handleKeyDown(event);

    await vi.waitFor(() => {
      expect(settingsApi.saveShortcuts).toHaveBeenCalledWith({
        toggle: "CommandOrControl+Shift+Space",
        push_to_talk: mockShortcuts.push_to_talk,
        rewrite: mockShortcuts.rewrite,
      });
    });
    expect(ctrl.recordingShortcut).toBe(false);
  });

  it("finishes by enabling auto-paste and marking setup complete", async () => {
    localStorage.setItem(PERMISSIONS_STORAGE_KEY, "1");
    const ctrl = createOnboardingController();
    await ctrl.mount();
    ctrl.autoPaste = true;

    // Skip to shortcut: mic → model → shortcut would require model ready.
    await ctrl.finish();

    expect(settingsApi.saveSettings).toHaveBeenCalledWith(
      expect.objectContaining({ auto_paste: true }),
    );
    expect(localStorage.getItem(SETUP_STORAGE_KEY)).toBe("1");
    expect(localStorage.getItem(PERMISSIONS_STORAGE_KEY)).toBe("1");
    expect(app.showOnboarding).toBe(false);
  });

  it("blocks continue on the model step until the model is ready", async () => {
    localStorage.setItem(PERMISSIONS_STORAGE_KEY, "1");
    const ctrl = createOnboardingController();
    await ctrl.mount();
    await ctrl.goNext(); // microphone → model
    expect(ctrl.step).toBe("model");
    expect(ctrl.canContinue).toBe(false);

    app.transcriptionRuntimePhase = "ready";
    expect(ctrl.canContinue).toBe(true);
  });
});
