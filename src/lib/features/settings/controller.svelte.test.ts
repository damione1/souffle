import { describe, it, expect, vi, beforeEach } from "vitest";

// --- Mocks for Tauri runtime ---

const { mockInvoke, mockListen, mockApplyTheme } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockListen: vi.fn(),
  mockApplyTheme: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => {
  class MockChannel {
    id = 1;
    onmessage: ((msg: unknown) => void) | null = null;
  }
  return {
    invoke: mockInvoke,
    Channel: MockChannel,
  };
});

vi.mock("@tauri-apps/api/event", () => ({
  listen: mockListen,
  once: vi.fn(),
  emit: vi.fn(),
}));

vi.mock("../../utils", async (importOriginal) => {
  const actual = await importOriginal<Record<string, unknown>>();
  return {
    ...actual,
    applyTheme: mockApplyTheme,
  };
});

import { createSettingsController } from "./controller.svelte";
import type {
  AppSettings,
  AudioDeviceInfo,
  ShortcutSettings,
  TranscriptionCatalog,
} from "../../types";

// --- Test fixtures ---

const defaultSettings: AppSettings = {
  theme: "dark",
  auto_paste: false,
  paste_delay_ms: 100,
  ollama_url: "http://localhost:11434",
  ollama_model: "",
  debug_transcription: false,
  audio_device: null,
  transcription_engine_id: "kyutai",
  transcription_model_id: "stt-1b-en_fr",
};

const fakeDevices: AudioDeviceInfo[] = [
  { name: "Built-in Microphone", is_default: true },
  { name: "USB Microphone", is_default: false },
];

const fakeShortcuts: ShortcutSettings = {
  toggle: "CommandOrControl+Shift+Space",
  push_to_talk: "",
};

const fakeCatalog: TranscriptionCatalog = {
  engines: [
    {
      id: "kyutai",
      label: "Kyutai",
      description: "Kyutai STT",
      supports_streaming: true,
      models: [
        {
          id: "stt-1b-en_fr",
          label: "STT 1B",
          description: "1B param model",
          download_size_bytes: 2400000000,
          supported_languages: ["en", "fr"],
        },
      ],
    },
  ],
  selected_engine_id: "kyutai",
  selected_model_id: "stt-1b-en_fr",
};

// --- Tests ---

describe("settings controller", () => {
  function defaultInvoke(cmd: string, _args?: Record<string, unknown>) {
    switch (cmd) {
      case "get_settings":
        return Promise.resolve(defaultSettings);
      case "save_settings":
        return Promise.resolve(null);
      case "get_shortcuts":
        return Promise.resolve(fakeShortcuts);
      case "save_shortcuts":
        return Promise.resolve(null);
      case "list_audio_devices":
        return Promise.resolve(fakeDevices);
      case "select_audio_device":
        return Promise.resolve(null);
      case "check_ollama":
        return Promise.resolve({ available: false, base_url: "http://localhost:11434", models: [] });
      case "get_transcription_catalog":
        return Promise.resolve(fakeCatalog);
      default:
        return Promise.resolve(null);
    }
  }

  beforeEach(() => {
    vi.clearAllMocks();
    mockInvoke.mockImplementation(defaultInvoke);
    mockListen.mockResolvedValue(vi.fn());
  });

  it("mount syncs all settings, shortcuts, devices", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    expect(mockInvoke).toHaveBeenCalledWith("get_settings");
    expect(mockInvoke).toHaveBeenCalledWith("get_shortcuts");
    expect(mockInvoke).toHaveBeenCalledWith("list_audio_devices");
    expect(mockInvoke).toHaveBeenCalledWith("check_ollama");
    expect(mockInvoke).toHaveBeenCalledWith("get_transcription_catalog");

    expect(ctrl.toggleShortcut).toBe("CommandOrControl+Shift+Space");
    expect(ctrl.pttShortcut).toBe("");
    expect(ctrl.audioDevices).toEqual(fakeDevices);
    expect(ctrl.catalog).toEqual(fakeCatalog);
    expect(mockApplyTheme).toHaveBeenCalledWith("dark");
  });

  it("onThemeChange persists", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    ctrl.onThemeChange("light");

    expect(mockApplyTheme).toHaveBeenCalledWith("light");
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_settings", expect.objectContaining({
        settings: expect.objectContaining({ theme: "light" }),
      }));
    });
  });

  it("onDeviceChange persists", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    const fakeEvent = {
      target: { value: "USB Microphone" },
    } as unknown as Event;

    await ctrl.onDeviceChange(fakeEvent);

    expect(mockInvoke).toHaveBeenCalledWith("select_audio_device", { deviceName: "USB Microphone" });
    expect(mockInvoke).toHaveBeenCalledWith("save_settings", expect.objectContaining({
      settings: expect.objectContaining({ audio_device: "USB Microphone" }),
    }));
  });

  it("shortcut recording flow", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    ctrl.startRecording("toggle");
    expect(ctrl.recordingField).toBe("toggle");

    const event = new KeyboardEvent("keydown", {
      key: "k",
      code: "KeyK",
      metaKey: true,
      shiftKey: true,
    });
    Object.defineProperty(event, "preventDefault", { value: vi.fn() });
    Object.defineProperty(event, "stopPropagation", { value: vi.fn() });

    ctrl.handleKeyDown(event);

    expect(ctrl.toggleShortcut).toBe("CommandOrControl+Shift+K");
    expect(ctrl.recordingField).toBeNull();
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_shortcuts", {
        shortcuts: expect.objectContaining({ toggle: "CommandOrControl+Shift+K" }),
      });
    });
  });

  it("plain key without modifier shows error", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    ctrl.startRecording("toggle");

    const event = new KeyboardEvent("keydown", {
      key: "a",
      code: "KeyA",
    });
    Object.defineProperty(event, "preventDefault", { value: vi.fn() });
    Object.defineProperty(event, "stopPropagation", { value: vi.fn() });

    ctrl.handleKeyDown(event);

    expect(ctrl.shortcutError).toContain("modifier key");
    expect(ctrl.recordingField).toBe("toggle");
  });

  it("escape cancels recording", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    const original = ctrl.toggleShortcut;
    ctrl.startRecording("toggle");
    expect(ctrl.recordingField).toBe("toggle");

    const event = new KeyboardEvent("keydown", { key: "Escape", code: "Escape" });
    Object.defineProperty(event, "preventDefault", { value: vi.fn() });
    Object.defineProperty(event, "stopPropagation", { value: vi.fn() });

    ctrl.handleKeyDown(event);

    expect(ctrl.recordingField).toBeNull();
    expect(ctrl.toggleShortcut).toBe(original);
  });

  it("backspace clears shortcut", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();
    expect(ctrl.toggleShortcut).toBe("CommandOrControl+Shift+Space");

    ctrl.startRecording("toggle");

    const event = new KeyboardEvent("keydown", { key: "Backspace", code: "Backspace" });
    Object.defineProperty(event, "preventDefault", { value: vi.fn() });
    Object.defineProperty(event, "stopPropagation", { value: vi.fn() });

    ctrl.handleKeyDown(event);

    expect(ctrl.toggleShortcut).toBe("");
    expect(ctrl.recordingField).toBeNull();
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("save_shortcuts", {
        shortcuts: expect.objectContaining({ toggle: "" }),
      });
    });
  });

  it("keyEvent requires modifier but allows function keys", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    ctrl.startRecording("ptt");

    const event = new KeyboardEvent("keydown", { key: "x", code: "KeyX" });
    Object.defineProperty(event, "preventDefault", { value: vi.fn() });
    Object.defineProperty(event, "stopPropagation", { value: vi.fn() });

    ctrl.handleKeyDown(event);

    expect(ctrl.shortcutError).toContain("modifier");
    expect(ctrl.recordingField).toBe("ptt");

    const fnEvent = new KeyboardEvent("keydown", { key: "F5", code: "F5" });
    Object.defineProperty(fnEvent, "preventDefault", { value: vi.fn() });
    Object.defineProperty(fnEvent, "stopPropagation", { value: vi.fn() });

    ctrl.handleKeyDown(fnEvent);

    expect(ctrl.pttShortcut).toBe("F5");
    expect(ctrl.recordingField).toBeNull();
  });

  it("formatShortcut converts symbols", () => {
    const ctrl = createSettingsController();

    expect(ctrl.formatShortcut("CommandOrControl+Shift+Space")).toBe("\u2318 \u21E7 Space");
    expect(ctrl.formatShortcut("Alt+K")).toBe("\u2325 K");
    expect(ctrl.formatShortcut("")).toBe("Not set");
  });
});
