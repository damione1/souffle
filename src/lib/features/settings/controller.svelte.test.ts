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
import { getAppState } from "../../stores/app.svelte";
import type {
  AppSettings,
  AudioDeviceInfo,
  ShortcutSettings,
  TranscriptionCatalog,
  TranscriptionRuntimeStatus,
} from "../../types";

// --- Test fixtures ---

const defaultSettings: AppSettings = {
  theme: "dark",
  locale: "",
  auto_paste: false,
  paste_delay_ms: 100,
  ollama_url: "http://localhost:11434",
  ollama_model: "",
  debug_transcription: false,
  audio_device: null,
  transcription_engine_id: "kyutai",
  transcription_model_id: "stt-1b-en_fr",
  transcription_backend_id: "candle",
  vad_enabled: true,
  filler_removal: true,
  stutter_collapse: false,
  dictionary_correction: true,
  capture_system_audio: true,
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
      models: [
        {
          id: "stt-1b-en_fr",
          label: "STT 1B",
          description: "1B param model",
          download_size_bytes: 2400000000,
          recommended_memory_bytes: 4000000000,
          supported_languages: ["en", "fr"],
          capabilities: {
            supports_streaming: true,
            supports_batch_transcription: false,
            supports_language_auto_detect: true,
            supports_word_timestamps: true,
            supports_partial_results: true,
          },
          audio_input: {
            sample_rate_hz: 24000,
            channels: 1,
            chunk_size_samples: 1920,
          },
          available_in_app: true,
          availability_note: null,
          backends: [
            {
              id: "candle",
              label: "Candle",
              description: "Pure Rust runtime",
              recommended: true,
              available_in_app: true,
              availability_note: null,
              artifacts: [],
            },
          ],
          recommended_backend_id: "candle",
        },
        {
          id: "stt-2.6b-en",
          label: "STT 2.6B",
          description: "2.6B param model",
          download_size_bytes: 6900000000,
          recommended_memory_bytes: 8000000000,
          supported_languages: ["en"],
          capabilities: {
            supports_streaming: true,
            supports_batch_transcription: false,
            supports_language_auto_detect: false,
            supports_word_timestamps: true,
            supports_partial_results: true,
          },
          audio_input: {
            sample_rate_hz: 24000,
            channels: 1,
            chunk_size_samples: 1920,
          },
          available_in_app: true,
          availability_note: null,
          backends: [
            {
              id: "candle",
              label: "Candle",
              description: "Pure Rust runtime",
              recommended: true,
              available_in_app: true,
              availability_note: null,
              artifacts: [],
            },
          ],
          recommended_backend_id: "candle",
        },
      ],
    },
  ],
  selected_engine_id: "kyutai",
  selected_model_id: "stt-1b-en_fr",
  selected_backend_id: "candle",
};

const fakeStatus: TranscriptionRuntimeStatus = {
  profile: {
    engine_id: "kyutai",
    engine_label: "Kyutai",
    model_id: "stt-1b-en_fr",
    model_label: "STT 1B",
    backend_id: "candle",
    backend_label: "Candle",
  },
  phase: "download_required",
  model_dir: "/tmp/models",
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
      case "get_model_status":
        return Promise.resolve(fakeStatus);
      default:
        return Promise.resolve(null);
    }
  }

  beforeEach(() => {
    vi.clearAllMocks();
    mockInvoke.mockImplementation(defaultInvoke);
    mockListen.mockResolvedValue(vi.fn());

    const app = getAppState();
    app.currentMeetingId = null;
    app.selectedDevice = "";
    app.machineState = { state: "idle" };
    app.transcriptionRuntimePhase = "download_required";
    app.downloadFile = "";
    app.downloadCompletedFiles = 0;
    app.downloadTotalFiles = 0;
    app.settings = { ...defaultSettings };
  });

  it("mount syncs all settings, shortcuts, devices", async () => {
    const ctrl = createSettingsController();
    await ctrl.mount();

    expect(mockInvoke).toHaveBeenCalledWith("get_settings");
    expect(mockInvoke).toHaveBeenCalledWith("get_shortcuts");
    expect(mockInvoke).toHaveBeenCalledWith("list_audio_devices");
    expect(mockInvoke).toHaveBeenCalledWith("check_ollama");
    expect(mockInvoke).toHaveBeenCalledWith("get_transcription_catalog");
    expect(mockInvoke).toHaveBeenCalledWith("get_model_status", {
      selection: {
        engine_id: "kyutai",
        model_id: "stt-1b-en_fr",
        backend_id: "candle",
      },
    });

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

  it("selectModelOption persists the profile and refreshes runtime state", async () => {
    let currentModelId = "stt-1b-en_fr";
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      switch (cmd) {
        case "get_settings":
          return Promise.resolve(defaultSettings);
        case "save_settings":
          currentModelId = (args?.settings as AppSettings).transcription_model_id;
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
          return Promise.resolve({
            ...fakeCatalog,
            selected_model_id: currentModelId,
          });
        case "get_model_status":
          return Promise.resolve(
            currentModelId === "stt-1b-en_fr"
              ? { ...fakeStatus, phase: "ready" }
              : {
                  ...fakeStatus,
                  profile: {
                    ...fakeStatus.profile,
                    model_id: "stt-2.6b-en",
                    model_label: "STT 2.6B",
                  },
                  phase: "download_required",
                },
          );
        default:
          return Promise.resolve(null);
      }
    });

    const ctrl = createSettingsController();
    await ctrl.mount();

    expect(ctrl.runtimePhase).toBe("ready");

    await ctrl.selectModelOption("kyutai:stt-2.6b-en");

    expect(ctrl.app.settings.transcription_model_id).toBe("stt-2.6b-en");
    expect(mockInvoke).toHaveBeenCalledWith("get_model_status", {
      selection: {
        engine_id: "kyutai",
        model_id: "stt-2.6b-en",
        backend_id: "candle",
      },
    });
    // The simple picker chains the download automatically.
    expect(mockInvoke).toHaveBeenCalledWith(
      "download_model",
      expect.objectContaining({
        selection: {
          engine_id: "kyutai",
          model_id: "stt-2.6b-en",
          backend_id: "candle",
        },
      }),
    );
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
