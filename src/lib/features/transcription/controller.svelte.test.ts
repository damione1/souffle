import { describe, it, expect, vi, beforeEach } from "vitest";

// --- Mocks for Tauri runtime ---

const { mockInvoke, mockListen } = vi.hoisted(() => ({
  mockInvoke: vi.fn(),
  mockListen: vi.fn(),
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

import { createTranscriptionController, notifyDictationStopRequested, resetTranscriptionControllerForTest } from "./controller.svelte";
import {
  startTranscriptionModelDownload,
  startTranscriptionModelLoad,
} from "./runtime";
import { getAppState } from "../../stores/app.svelte";
import { mockSettings } from "../../test-helpers/fixtures";
import type {
  TranscriptionCatalog,
  TranscriptionRuntimeStatus,
  DictationEntry,
} from "../../types";

// --- Test fixtures ---

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
  phase: "ready",
  model_dir: "/tmp/models",
};

const fakeHistory: DictationEntry[] = [
  { id: "1", text: "Hello world", timestamp: "2025-01-01T00:00:00Z" },
  { id: "2", text: "Second entry", timestamp: "2025-01-01T01:00:00Z" },
];

// --- Tests ---

describe("transcription controller", () => {
  const mockUnlisten = vi.fn();
  const eventListeners: Record<string, (event: unknown) => void> = {};
  const selection = {
    engine_id: "kyutai",
    model_id: "stt-1b-en_fr",
    backend_id: "candle",
  };
  let nowOffset = 0;
  const realDateNow = Date.now.bind(Date);

  function defaultInvoke(cmd: string, args?: Record<string, unknown>) {
    switch (cmd) {
      case "get_transcription_catalog":
        return Promise.resolve(fakeCatalog);
      case "get_model_status":
        return Promise.resolve(fakeStatus);
      case "list_dictation_entries":
        return Promise.resolve(fakeHistory);
      case "start_transcription":
        return Promise.resolve(null);
      case "stop_transcription":
        return Promise.resolve(null);
      case "add_dictation_entry":
        return Promise.resolve("entry-test-id");
      case "update_dictation_entry":
        return Promise.resolve(null);
      case "delete_dictation_entry":
        return Promise.resolve(null);
      case "list_snippets":
        return Promise.resolve([]);
      case "clear_dictation_history":
        return Promise.resolve(null);
      case "paste_text":
        return Promise.resolve(null);
      case "copy_text":
        return Promise.resolve(null);
      case "polish_dictation":
        return Promise.resolve({ text: args?.text ?? "", skipped: true, warning: null });
      case "pill_hold":
        return Promise.resolve(null);
      case "pill_release":
        return Promise.resolve(null);
      case "load_model":
        return Promise.resolve(null);
      case "download_model":
        return Promise.resolve(null);
      case "save_settings":
        return Promise.resolve(null);
      case "frontmost_app_name":
        return Promise.resolve(null);
      case "read_selected_text":
        return Promise.resolve(null);
      case "read_focused_text":
        return Promise.resolve(null);
      case "learn_from_edit":
        return Promise.resolve(0);
      default:
        return Promise.resolve(null);
    }
  }

  beforeEach(() => {
    vi.clearAllMocks();
    resetTranscriptionControllerForTest();
    for (const key of Object.keys(eventListeners)) delete eventListeners[key];
    mockInvoke.mockImplementation(defaultInvoke);
    mockListen.mockImplementation((event: string, cb: (event: unknown) => void) => {
      eventListeners[event] = cb;
      return Promise.resolve(mockUnlisten);
    });

    nowOffset = 0;
    vi.spyOn(Date, "now").mockImplementation(() => realDateNow() + nowOffset);

    // Reset shared singleton app state between tests
    const app = getAppState();
    app.currentMeetingId = null;
    app.machineState = { state: "idle" };
    app.transcriptionRuntimePhase = "download_required";
    app.downloadFile = "";
    app.downloadCompletedFiles = 0;
    app.downloadTotalFiles = 0;
    app.selectedDevice = "";
    app.settings = { ...mockSettings };
    app.settingsOpen = false;
    app.settingsInitialTab = null;
    app.permissionsPanelOpen = false;
    app.snippets = [];

    Object.assign(navigator, {
      clipboard: { writeText: vi.fn().mockResolvedValue(undefined) },
    });
  });

  it("mount loads catalog and runtime status", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    expect(mockInvoke).toHaveBeenCalledWith("get_transcription_catalog");
    expect(mockInvoke).toHaveBeenCalledWith("get_model_status", { selection });
    expect(ctrl.runtimePhase).toBe("ready");
  });

  /** Simulate the backend emitting a StateChanged event by setting machineState */
  function simulateRecordingStarted(
    app: ReturnType<typeof getAppState>,
    elapsedMs = 600,
  ) {
    app.machineState = { state: "recording_dictation", data: { profile: { engine_id: "kyutai", engine_label: "Kyutai", model_id: "stt-1b-en_fr", model_label: "STT 1B", backend_id: "candle", backend_label: "Candle" }, session_id: 1 } };
    nowOffset += elapsedMs;
  }

  it("toggleRecording starts when loaded", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await ctrl.toggleRecording();

    expect(mockInvoke).toHaveBeenCalledWith("start_transcription", expect.objectContaining({ channel: expect.any(Object) }));
  });

  it("toggleRecording stop saves to history", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await ctrl.toggleRecording();
    // Simulate backend state change
    simulateRecordingStarted(ctrl.app);
    expect(ctrl.app.isRecording).toBe(true);

    await ctrl.toggleRecording();

    expect(mockInvoke).toHaveBeenCalledWith("stop_transcription");
  });

  it("toggleRecording stop auto-pastes when fromShortcut and auto_paste enabled", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    ctrl.app.settings = { ...ctrl.app.settings, auto_paste: true, paste_delay_ms: 50 };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    expect(ctrl.app.isRecording).toBe(true);

    // Stop with fromShortcut=true — transcript is "" so paste won't trigger for empty text
    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("stop_transcription");
    // pasteText is NOT called because transcript is empty (Channel is mocked)
    expect(mockInvoke).not.toHaveBeenCalledWith("paste_text", expect.anything());
  });

  // SOU-053: a shortcut dictation runs from another app, so the in-app
  // status banner alone would go unseen on a failed paste. The controller
  // must also fire a system notification through the backend.
  it("notifies outside the window when a shortcut paste fails", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "paste_text") {
        return Promise.reject("Accessibility permission missing.");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: false,
    };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("copy_text", expect.anything());
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    expect(mockInvoke).toHaveBeenCalledWith("notify_paste_failed", {
      error: "Accessibility permission missing.",
      savedToHistory: true,
    });
    expect(ctrl.statusMessage).toBe("Copied — press ⌘V");
    expect(ctrl.statusActionLabel).toBe("Repair permission");
    expect(ctrl.statusAction).toBeTypeOf("function");
    ctrl.statusAction?.();
    expect(ctrl.app.permissionsPanelOpen).toBe(true);
  });

  it("notifies outside the window when a shortcut paste fails and history fails", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "paste_text") {
        return Promise.reject("Accessibility permission missing.");
      }
      if (cmd === "add_dictation_entry") {
        return Promise.reject("DB error");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: false,
    };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("copy_text", expect.anything());
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    expect(mockInvoke).toHaveBeenCalledWith("notify_paste_failed", {
      error: "Accessibility permission missing.",
      savedToHistory: false,
    });
  });

  it("does not notify when a shortcut paste succeeds", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: false,
    };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("copy_text", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("notify_paste_failed", expect.anything());
  });

  it("copies via Rust and reports paste_failed when paste fails for a non-AX reason", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "paste_text") {
        return Promise.reject("Enigo init: some OS error");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: false,
    };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.anything());
    expect(mockInvoke).toHaveBeenCalledWith("copy_text", { text: "hello world" });
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    expect(ctrl.statusMessage).toBe("Paste failed: Enigo init: some OS error");
    expect(ctrl.statusActionLabel).toBeUndefined();
    expect(mockInvoke).toHaveBeenCalledWith("notify_paste_failed", {
      error: "Enigo init: some OS error",
      savedToHistory: true,
    });
  });

  it("does not claim Copied when Accessibility paste fails after a failed copy", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "paste_text") {
        return Promise.reject("Accessibility permission missing. (no pasteboard)");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: false,
    };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("copy_text", { text: "hello world" });
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    expect(ctrl.statusMessage).toBe(
      "Paste failed: Accessibility permission missing. (no pasteboard)",
    );
    expect(ctrl.statusActionLabel).toBeUndefined();
  });

  it("toggleRecording stop skips polish IPC when polish disabled", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: false };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording();

    expect(mockInvoke).toHaveBeenCalledWith("stop_transcription");
    expect(mockInvoke).not.toHaveBeenCalledWith("polish_dictation", expect.anything());
    expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello world" });
  });

  it("persists raw history before polish returns (SOU-048)", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    let resolvePolish: ((value: unknown) => void) | undefined;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "polish_dictation") {
        return new Promise((resolve) => {
          resolvePolish = resolve;
        });
      }
      if (cmd === "add_dictation_entry") {
        return Promise.resolve("raw-id");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    const stopPromise = ctrl.toggleRecording();
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello world" });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("update_dictation_entry", expect.anything());
    expect(ctrl.isStopping).toBe(true);

    await vi.waitFor(() => {
      expect(resolvePolish).toBeTypeOf("function");
    });
    resolvePolish!({ text: "hello world", skipped: false, warning: null });
    await stopPromise;
    expect(mockInvoke).not.toHaveBeenCalledWith("update_dictation_entry", expect.anything());
  });

  it("updates the same history row when polish returns different text (SOU-048)", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "polish_dictation") {
        return Promise.resolve({ text: "Hello, world.", skipped: false, warning: null });
      }
      if (cmd === "add_dictation_entry") {
        return Promise.resolve("raw-id");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    await ctrl.toggleRecording();

    const addCalls = mockInvoke.mock.calls.filter(([cmd]) => cmd === "add_dictation_entry");
    const updateCalls = mockInvoke.mock.calls.filter(([cmd]) => cmd === "update_dictation_entry");
    expect(addCalls).toHaveLength(1);
    expect(addCalls[0][1]).toEqual({ text: "hello world" });
    expect(updateCalls).toHaveLength(1);
    expect(updateCalls[0][1]).toEqual({ id: "raw-id", text: "Hello, world." });
  });

  it("polish timeout returns a warning and still saved raw (SOU-048)", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    let sawAdd: () => void = () => {};
    const addSeen = new Promise<void>((resolve) => {
      sawAdd = resolve;
    });
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "polish_dictation") {
        return new Promise(() => {});
      }
      if (cmd === "add_dictation_entry") {
        sawAdd();
        return Promise.resolve("raw-id");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const stopPromise = ctrl.toggleRecording();
      await addSeen;
      expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello world" });

      await vi.advanceTimersByTimeAsync(25_000);
      await stopPromise;

      expect(ctrl.statusMessage).toBe("Polish took too long. Saved the original text.");
      expect(mockInvoke).not.toHaveBeenCalledWith("update_dictation_entry", expect.anything());
    } finally {
      vi.useRealTimers();
    }
  });

  it("late polish rejection after timeout is swallowed (SOU-048)", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    let rejectPolish: (err: unknown) => void = () => {};
    let sawAdd: () => void = () => {};
    const addSeen = new Promise<void>((resolve) => {
      sawAdd = resolve;
    });
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "polish_dictation") {
        return new Promise((_, reject) => {
          rejectPolish = reject;
        });
      }
      if (cmd === "add_dictation_entry") {
        sawAdd();
        return Promise.resolve("raw-id");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });

    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const stopPromise = ctrl.toggleRecording();
      await addSeen;
      await vi.advanceTimersByTimeAsync(25_000);
      await stopPromise;

      rejectPolish(new Error("provider down"));
      await new Promise<void>((resolve) => queueMicrotask(resolve));
      await new Promise<void>((resolve) => queueMicrotask(resolve));
      expect(ctrl.statusMessage).toBe("Polish took too long. Saved the original text.");
    } finally {
      vi.useRealTimers();
    }
  });

  it("toggleRecording stop holds the pill before stopping when polish is enabled, then releases it", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    await ctrl.toggleRecording();

    const holdIndex = mockInvoke.mock.calls.findIndex((call) => call[0] === "pill_hold");
    const stopIndex = mockInvoke.mock.calls.findIndex((call) => call[0] === "stop_transcription");
    const releaseIndex = mockInvoke.mock.calls.findIndex((call) => call[0] === "pill_release");

    expect(holdIndex).toBeGreaterThanOrEqual(0);
    expect(stopIndex).toBeGreaterThan(holdIndex);
    expect(releaseIndex).toBeGreaterThan(stopIndex);
    expect(mockInvoke).toHaveBeenCalledWith("pill_hold", { kind: "polishing" });
  });

  it("toggleRecording stop never holds the pill when polish is disabled", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: false };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    await ctrl.toggleRecording();

    expect(mockInvoke).not.toHaveBeenCalledWith("pill_hold", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("pill_release", expect.anything());
  });

  it("toggleRecording stop releases the pill hold even when the stop pipeline throws", async () => {
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "stop_transcription") {
        return Promise.reject(new Error("drain failed"));
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    await ctrl.toggleRecording();

    expect(mockInvoke).toHaveBeenCalledWith("pill_hold", { kind: "polishing" });
    expect(mockInvoke).toHaveBeenCalledWith("pill_release");
    expect(ctrl.statusMessage).toContain("drain failed");
  });

  it("toggleRecording stop clipboard only when not fromShortcut", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    await ctrl.toggleRecording();

    // No paste or clipboard since transcript is empty
    expect(mockInvoke).not.toHaveBeenCalledWith("paste_text", expect.anything());
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
  });

  it("toggleRecording not loaded shows message", async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_model_status") {
        return Promise.resolve({ ...fakeStatus, phase: "download_required" });
      }
      return defaultInvoke(cmd);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await ctrl.toggleRecording();

    expect(mockInvoke).not.toHaveBeenCalledWith("start_transcription", expect.anything());
    expect(ctrl.statusMessage).toContain("Download and load");
    expect(ctrl.statusActionLabel).toBe("Open model");
    ctrl.statusAction?.();
    expect(ctrl.app.settingsOpen).toBe(true);
    expect(ctrl.app.settingsInitialTab).toBe("transcription");
  });

  it("toggleRecording guards double start", async () => {
    let resolveStart: (() => void) | undefined;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "start_transcription") {
        return new Promise<void>((r) => { resolveStart = r; });
      }
      return defaultInvoke(cmd);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();

    const first = ctrl.toggleRecording();
    const second = ctrl.toggleRecording();

    await vi.waitFor(() => {
      expect(resolveStart).toBeTypeOf("function");
    });
    resolveStart!();
    await first;
    await second;

    const startCalls = mockInvoke.mock.calls.filter((call) => call[0] === "start_transcription");
    expect(startCalls).toHaveLength(1);
  });

  it("clears a stale Repair action at the start of a start attempt", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "paste_text") {
        return Promise.reject("Accessibility permission missing.");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: false,
    };
    ctrl.app.transcriptionRuntimePhase = "ready";

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });
    await ctrl.toggleRecording(true);

    expect(mockInvoke).not.toHaveBeenCalledWith("copy_text", expect.anything());
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
    expect(ctrl.statusActionLabel).toBe("Repair permission");

    ctrl.app.machineState = { state: "idle" };
    ctrl.app.transcriptionRuntimePhase = "download_required";
    await ctrl.toggleRecording();

    expect(ctrl.statusMessage).toContain("Download and load");
    expect(ctrl.statusActionLabel).toBe("Open model");
    expect(ctrl.statusActionLabel).not.toBe("Repair permission");
  });

  it("does not double-start while ensureModelLoaded is still pending", async () => {
    let resolveLoad: (() => void) | undefined;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_model_status") {
        return Promise.resolve({ ...fakeStatus, phase: "load_required" });
      }
      if (cmd === "load_model") {
        return new Promise<void>((r) => {
          resolveLoad = r;
        });
      }
      return defaultInvoke(cmd);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.transcriptionRuntimePhase = "load_required";

    const first = ctrl.toggleRecording();
    const second = ctrl.toggleRecording();

    await vi.waitFor(() => {
      expect(resolveLoad).toBeTypeOf("function");
    });
    expect(mockInvoke.mock.calls.filter((call) => call[0] === "load_model")).toHaveLength(1);

    resolveLoad!();
    await first;
    await second;

    expect(mockInvoke.mock.calls.filter((call) => call[0] === "load_model")).toHaveLength(1);
  });

  it("model download (runtime) tracks progress", async () => {
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "get_model_status") {
        return Promise.resolve({ ...fakeStatus, phase: "load_required" });
      }
      if (cmd === "download_model") {
        const channel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        if (channel?.onmessage) {
          channel.onmessage({
            file: "model.safetensors",
            downloaded_bytes: 500,
            total_bytes: 1000,
            completed_files: 1,
            total_files: 4,
            status: "downloading",
          });
          channel.onmessage({
            file: "all",
            downloaded_bytes: 0,
            total_bytes: null,
            completed_files: 4,
            total_files: 4,
            status: "complete",
          });
        }
        return Promise.resolve(null);
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await startTranscriptionModelDownload(ctrl.app, ctrl.catalog, () => {});

    expect(mockInvoke).toHaveBeenCalledWith(
      "download_model",
      expect.objectContaining({ selection, channel: expect.any(Object) }),
    );
    expect(ctrl.modelOperationState).toBe("idle");
    expect(ctrl.downloadCompletedFiles).toBe(4);
    expect(ctrl.downloadTotalFiles).toBe(4);
  });

  it("model load (runtime) sets runtimePhase to ready", async () => {
    let statusCallCount = 0;
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === "get_model_status") {
        statusCallCount++;
        return Promise.resolve(statusCallCount <= 1
          ? { ...fakeStatus, phase: "load_required" }
          : fakeStatus,
        );
      }
      return defaultInvoke(cmd);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    expect(ctrl.runtimePhase).toBe("load_required");

    await startTranscriptionModelLoad(ctrl.app, ctrl.catalog, () => {});

    expect(mockInvoke).toHaveBeenCalledWith("load_model", { selection });
    expect(ctrl.runtimePhase).toBe("ready");
    expect(ctrl.modelOperationState).toBe("idle");
  });

  it("insert start polishes with focusedApp", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "frontmost_app_name") return Promise.resolve("Mail");
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    expect(mockInvoke).toHaveBeenCalledWith("frontmost_app_name");
    expect(mockInvoke).not.toHaveBeenCalledWith("read_selected_text");

    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
      start_ms: 0,
      end_ms: 1000,
    });
    await ctrl.toggleRecording();

    expect(mockInvoke).toHaveBeenCalledWith("polish_dictation", expect.objectContaining({
      text: "hello world",
      focusedApp: "Mail",
    }));
  });

  it("learn_from_edit runs after auto-paste when focused text changed", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
      mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "start_transcription") {
          transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
          return Promise.resolve(null);
        }
        if (cmd === "frontmost_app_name") return Promise.resolve("Notes");
        if (cmd === "read_focused_text") return Promise.resolve("hello Kubernetes");
        if (cmd === "learn_from_edit") return Promise.resolve(1);
        return defaultInvoke(cmd, args);
      });

      const ctrl = createTranscriptionController();
      await ctrl.mount();
      ctrl.app.settings = {
        ...ctrl.app.settings,
        auto_paste: true,
        dictation_polish_enabled: false,
        dictation_learn_from_edit: true,
      };

      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app);
      (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
        text: "hello world",
        is_final: true,
        start_ms: 0,
        end_ms: 1000,
      });
      await ctrl.toggleRecording(true);

      expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.anything());
      expect(mockInvoke).not.toHaveBeenCalledWith("learn_from_edit", expect.anything());

      await vi.advanceTimersByTimeAsync(4000);
      expect(mockInvoke).toHaveBeenCalledWith("read_focused_text");
      expect(mockInvoke).toHaveBeenCalledWith("learn_from_edit", {
        original: "hello world",
        corrected: "hello Kubernetes",
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("learn_from_edit skips when the focused app changed (SOU-047)", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
      let frontmost = "Notes";
      mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "start_transcription") {
          transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
          return Promise.resolve(null);
        }
        if (cmd === "frontmost_app_name") return Promise.resolve(frontmost);
        if (cmd === "read_focused_text") return Promise.resolve("hello Kubernetes");
        if (cmd === "learn_from_edit") return Promise.resolve(1);
        return defaultInvoke(cmd, args);
      });

      const ctrl = createTranscriptionController();
      await ctrl.mount();
      ctrl.app.settings = {
        ...ctrl.app.settings,
        auto_paste: true,
        dictation_polish_enabled: false,
        dictation_learn_from_edit: true,
      };

      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app);
      (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
        text: "hello world",
        is_final: true,
        start_ms: 0,
        end_ms: 1000,
      });
      await ctrl.toggleRecording(true);
      frontmost = "Safari";

      await vi.advanceTimersByTimeAsync(4000);
      expect(mockInvoke).not.toHaveBeenCalledWith("learn_from_edit", expect.anything());
    } finally {
      vi.useRealTimers();
    }
  });

  it("learn_from_edit skips when the focused app cannot be identified (SOU-047)", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
      let frontmost: string | null = "Notes";
      mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "start_transcription") {
          transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
          return Promise.resolve(null);
        }
        if (cmd === "frontmost_app_name") return Promise.resolve(frontmost);
        if (cmd === "read_focused_text") return Promise.resolve("hello Kubernetes");
        if (cmd === "learn_from_edit") return Promise.resolve(1);
        return defaultInvoke(cmd, args);
      });

      const ctrl = createTranscriptionController();
      await ctrl.mount();
      ctrl.app.settings = {
        ...ctrl.app.settings,
        auto_paste: true,
        dictation_polish_enabled: false,
        dictation_learn_from_edit: true,
      };

      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app);
      (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
        text: "hello world",
        is_final: true,
        start_ms: 0,
        end_ms: 1000,
      });
      await ctrl.toggleRecording(true);
      frontmost = null;

      await vi.advanceTimersByTimeAsync(4000);
      expect(mockInvoke).not.toHaveBeenCalledWith("learn_from_edit", expect.anything());
    } finally {
      vi.useRealTimers();
    }
  });

  it("learn_from_edit skips when the paste target was never captured (SOU-047)", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
      mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
        if (cmd === "start_transcription") {
          transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
          return Promise.resolve(null);
        }
        if (cmd === "frontmost_app_name") return Promise.resolve(null);
        if (cmd === "read_focused_text") return Promise.resolve("hello Kubernetes");
        if (cmd === "learn_from_edit") return Promise.resolve(1);
        return defaultInvoke(cmd, args);
      });

      const ctrl = createTranscriptionController();
      await ctrl.mount();
      ctrl.app.settings = {
        ...ctrl.app.settings,
        auto_paste: true,
        dictation_polish_enabled: false,
        dictation_learn_from_edit: true,
      };

      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app);
      (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
        text: "hello world",
        is_final: true,
        start_ms: 0,
        end_ms: 1000,
      });
      await ctrl.toggleRecording(true);

      await vi.advanceTimersByTimeAsync(4000);
      expect(mockInvoke).not.toHaveBeenCalledWith("learn_from_edit", expect.anything());
    } finally {
      vi.useRealTimers();
    }
  });

  function captureTranscriptionChannel() {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      return defaultInvoke(cmd, args);
    });
    return {
      emit(segment: { text: string; is_final: boolean }) {
        transcriptionChannel?.onmessage?.(segment);
      },
    };
  }

  it("non-final sets tentative without changing transcript", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    await ctrl.toggleRecording();

    channel.emit({ text: "hello", is_final: false });

    expect(ctrl.tentative).toBe("hello");
    expect(ctrl.transcript).toBe("");
  });

  it("final after tentative clears tentative and appends to transcript", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    await ctrl.toggleRecording();

    channel.emit({ text: "hello", is_final: true });
    channel.emit({ text: "world", is_final: false });
    expect(ctrl.transcript).toBe("hello");
    expect(ctrl.tentative).toBe("world");

    channel.emit({ text: "world", is_final: true });
    expect(ctrl.tentative).toBe("");
    expect(ctrl.transcript).toBe("hello world");
  });

  it("paste and polish use transcript only, not the live tentative", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true, auto_paste: true };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    channel.emit({ text: "hello", is_final: true });
    channel.emit({ text: "pending", is_final: false });
    expect(ctrl.tentative).toBe("pending");

    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("polish_dictation", expect.objectContaining({
      text: "hello",
    }));
    expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello" });
    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.objectContaining({
      text: "hello",
    }));
    expect(mockInvoke).not.toHaveBeenCalledWith("polish_dictation", expect.objectContaining({
      text: expect.stringContaining("pending"),
    }));
  });

  it("shortcut start then window stop still auto-pastes (SOU-046)", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, auto_paste: true, dictation_polish_enabled: false };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    channel.emit({ text: "hello", is_final: true });
    await ctrl.toggleRecording(false);

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.objectContaining({ text: "hello" }));
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
  });

  it("window start then shortcut stop does not auto-paste (SOU-046)", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, auto_paste: true, dictation_polish_enabled: false };

    await ctrl.toggleRecording(false);
    simulateRecordingStarted(ctrl.app);
    channel.emit({ text: "hello", is_final: true });
    await ctrl.toggleRecording(true);

    expect(mockInvoke).not.toHaveBeenCalledWith("paste_text", expect.anything());
    expect(navigator.clipboard.writeText).toHaveBeenCalledWith("hello");
  });

  it("abort during finalization still pastes a shortcut-started session (SOU-046)", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    let resolvePolish: ((value: unknown) => void) | undefined;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "polish_dictation") {
        return new Promise((resolve) => {
          resolvePolish = resolve;
        });
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = {
      ...ctrl.app.settings,
      auto_paste: true,
      dictation_polish_enabled: true,
    };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello",
      is_final: true,
      start_ms: 0,
      end_ms: 500,
    });

    const stopPromise = ctrl.toggleRecording(false);
    await vi.waitFor(() => {
      expect(resolvePolish).toBeTypeOf("function");
    });
    ctrl.handleRecordingAborted();
    resolvePolish!({ text: "hello", skipped: false, warning: null });
    await stopPromise;

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.objectContaining({ text: "hello" }));
    expect(navigator.clipboard.writeText).not.toHaveBeenCalled();
  });

  it("abort clears tentative and saves transcript only", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: false };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    channel.emit({ text: "hello", is_final: true });
    channel.emit({ text: "pending", is_final: false });
    expect(ctrl.tentative).toBe("pending");

    ctrl.handleRecordingAborted();

    expect(ctrl.tentative).toBe("");
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello" });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("add_dictation_entry", {
      text: expect.stringContaining("pending"),
    });
  });

  it("abort persists raw before polish returns (SOU-048)", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    let resolvePolish: ((value: unknown) => void) | undefined;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      if (cmd === "polish_dictation") {
        return new Promise((resolve) => {
          resolvePolish = resolve;
        });
      }
      if (cmd === "add_dictation_entry") {
        return Promise.resolve("raw-id");
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello",
      is_final: true,
      start_ms: 0,
      end_ms: 500,
    });

    ctrl.handleRecordingAborted();

    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello" });
    });
    expect(mockInvoke).not.toHaveBeenCalledWith("update_dictation_entry", expect.anything());

    await vi.waitFor(() => {
      expect(resolvePolish).toBeTypeOf("function");
    });
    resolvePolish!({ text: "Hello.", skipped: false, warning: null });
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("update_dictation_entry", {
        id: "raw-id",
        text: "Hello.",
      });
    });
    const addCalls = mockInvoke.mock.calls.filter(([cmd]) => cmd === "add_dictation_entry");
    expect(addCalls).toHaveLength(1);
  });

  it("starting a session resets leftover tentative", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    channel.emit({ text: "stale", is_final: false });
    expect(ctrl.tentative).toBe("stale");

    await ctrl.toggleRecording();
    ctrl.app.machineState = { state: "idle" };
    await ctrl.toggleRecording();
    expect(ctrl.tentative).toBe("");
    expect(ctrl.transcript).toBe("");
  });

  it("short dictation clears tentative and skips paste", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, auto_paste: true };

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app, 0);
    channel.emit({ text: "pending", is_final: false });
    expect(ctrl.tentative).toBe("pending");

    await ctrl.toggleRecording(true);

    expect(ctrl.tentative).toBe("");
    expect(ctrl.transcript).toBe("");
    expect(ctrl.statusMessage).toBe("Hold a little longer");
    expect(mockInvoke).toHaveBeenCalledWith("stop_transcription");
    expect(mockInvoke).not.toHaveBeenCalledWith("paste_text", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("add_dictation_entry", expect.anything());
  });

  it("does not auto-hide a banner that carries an action", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      mockInvoke.mockImplementation((cmd: string) => {
        if (cmd === "get_model_status") {
          return Promise.resolve({ ...fakeStatus, phase: "download_required" });
        }
        return defaultInvoke(cmd);
      });

      const ctrl = createTranscriptionController();
      await ctrl.mount();
      await ctrl.toggleRecording();

      expect(ctrl.statusActionLabel).toBe("Open model");
      const message = ctrl.statusMessage;
      expect(message).toContain("Download and load");

      await vi.advanceTimersByTimeAsync(5000);
      expect(ctrl.statusMessage).toBe(message);
      expect(ctrl.statusActionLabel).toBe("Open model");
    } finally {
      vi.useRealTimers();
    }
  });

  it("clearBanner dismisses immediately and cancels the auto-hide timer", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const ctrl = createTranscriptionController();
      await ctrl.mount();

      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app, 0);
      await ctrl.toggleRecording(true);
      expect(ctrl.statusMessage).toBe("Hold a little longer");

      ctrl.clearBanner();
      expect(ctrl.statusMessage).toBe("");
      expect(ctrl.statusActionLabel).toBeUndefined();

      await vi.advanceTimersByTimeAsync(5000);
      expect(ctrl.statusMessage).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("auto-hides the banner after 5s without wiping a later banner", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const ctrl = createTranscriptionController();
      await ctrl.mount();

      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app, 0);
      await ctrl.toggleRecording(true);
      expect(ctrl.statusMessage).toBe("Hold a little longer");

      await vi.advanceTimersByTimeAsync(5000);
      expect(ctrl.statusMessage).toBe("");

      // Test AC4: A later banner cancels the previous timer
      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app, 0);
      await ctrl.toggleRecording(true);
      expect(ctrl.statusMessage).toBe("Hold a little longer");
      
      await vi.advanceTimersByTimeAsync(2000);
      
      // Simulate another banner before timeout
      ctrl.app.transcriptionRuntimePhase = "download_required";
      await vi.advanceTimersByTimeAsync(0); // wait for reactivity
      // In reality, download_required might trigger modelRequiredBanner via notifyDictationAborted,
      // but let us just call setBanner directly if we could. Since we cannot access setBanner,
      // we can trigger another short PTT.
      await ctrl.toggleRecording(true);
      simulateRecordingStarted(ctrl.app, 0);
      await ctrl.toggleRecording(true);
      
      // We wait 4000ms. If the first timer was not cancelled, it would fire (2000+4000 > 5000)
      // and clear the banner.
      await vi.advanceTimersByTimeAsync(4000);
      expect(ctrl.statusMessage).toBe("Hold a little longer"); // second banner is still here
      
      await vi.advanceTimersByTimeAsync(2000);
      expect(ctrl.statusMessage).toBe(""); // finally clears
    } finally {
      vi.useRealTimers();
    }
  });

  it("dictation ceiling stops a long session", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    try {
      const ctrl = createTranscriptionController();
      await ctrl.mount();
      ctrl.app.settings = { ...ctrl.app.settings, dictation_ceiling_seconds: 2 };

      await ctrl.toggleRecording();
      simulateRecordingStarted(ctrl.app);

      await vi.advanceTimersByTimeAsync(5000);
      await vi.waitFor(() => {
        expect(mockInvoke).toHaveBeenCalledWith("stop_transcription");
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("shortcut-toggle is a no-op while a meeting is recording (SOU-044)", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    ctrl.app.machineState = {
      state: "recording_meeting",
      data: {
        profile: {
          engine_id: "kyutai",
          engine_label: "Kyutai",
          model_id: "stt-1b-en_fr",
          model_label: "STT 1B",
          backend_id: "candle",
          backend_label: "Candle",
        },
        session_id: 1,
        meeting_id: "meeting-1",
      },
    };

    eventListeners["shortcut-toggle"]?.({ payload: null });

    expect(mockInvoke).not.toHaveBeenCalledWith("stop_transcription");
    expect(mockInvoke).not.toHaveBeenCalledWith("start_transcription", expect.anything());
    // The meeting's own state must survive untouched.
    expect(ctrl.app.machineState.state).toBe("recording_meeting");
  });

  it("notifyDictationStopRequested stops an active dictation (HUD stop)", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    expect(ctrl.app.recordingMode).toBe("dictation");

    notifyDictationStopRequested();
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("stop_transcription");
    });
  });

  it("notifyDictationStopRequested is a no-op while a meeting is recording (SOU-044)", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    ctrl.app.machineState = {
      state: "recording_meeting",
      data: {
        profile: {
          engine_id: "kyutai",
          engine_label: "Kyutai",
          model_id: "stt-1b-en_fr",
          model_label: "STT 1B",
          backend_id: "candle",
          backend_label: "Candle",
        },
        session_id: 1,
        meeting_id: "meeting-1",
      },
    };

    notifyDictationStopRequested();

    expect(mockInvoke).not.toHaveBeenCalledWith("stop_transcription");
    expect(mockInvoke).not.toHaveBeenCalledWith("start_transcription", expect.anything());
    expect(ctrl.app.machineState.state).toBe("recording_meeting");
  });

  it("a stopped dictation's transcript cannot be repasted by a later interrupted session (SOU-044)", async () => {
    const channel = captureTranscriptionChannel();
    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, auto_paste: true, dictation_polish_enabled: false };

    // Session 1: dictate "hello" and stop via the shortcut path (auto-paste fires).
    await ctrl.toggleRecording(true);
    simulateRecordingStarted(ctrl.app);
    channel.emit({ text: "hello", is_final: true });
    await ctrl.toggleRecording(true);

    expect(mockInvoke).toHaveBeenCalledWith("paste_text", expect.objectContaining({ text: "hello" }));
    expect(ctrl.transcript).toBe("");
    ctrl.app.machineState = { state: "idle" };

    // Session 2: start fresh, then get interrupted before any words arrive.
    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    ctrl.handleRecordingAborted();

    expect(ctrl.transcript).toBe("");
    const pasteCalls = mockInvoke.mock.calls.filter(([cmd]) => cmd === "paste_text");
    expect(pasteCalls).toHaveLength(1);
    expect(pasteCalls[0][1]).toEqual(expect.objectContaining({ text: "hello" }));
  });

  it("queued PTT stop does not stop a later dictation after abort (SOU-045)", async () => {
    let releaseStart: (() => void) | undefined;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        return new Promise<void>((r) => { releaseStart = r; });
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();

    const firstStart = ctrl.toggleRecording(true);
    await vi.waitFor(() => {
      expect(releaseStart).toBeTypeOf("function");
    });

    eventListeners["shortcut-ptt-stop"]?.({ payload: null });
    releaseStart!();
    await firstStart;
    simulateRecordingStarted(ctrl.app);

    ctrl.handleRecordingAborted();
    ctrl.app.machineState = { state: "idle" };

    mockInvoke.mockImplementation(defaultInvoke);
    await ctrl.toggleRecording(true);
    const startCalls = mockInvoke.mock.calls.filter((call) => call[0] === "start_transcription");
    expect(startCalls).toHaveLength(2);
    simulateRecordingStarted(ctrl.app);

    await new Promise((r) => setTimeout(r, 60));

    expect(mockInvoke).not.toHaveBeenCalledWith("stop_transcription");
  });

  it("toggleRecording directly is a no-op while a meeting is recording", async () => {
    const ctrl = createTranscriptionController();
    await ctrl.mount();

    ctrl.app.machineState = {
      state: "recording_meeting",
      data: {
        profile: {
          engine_id: "kyutai",
          engine_label: "Kyutai",
          model_id: "stt-1b-en_fr",
          model_label: "STT 1B",
          backend_id: "candle",
          backend_label: "Candle",
        },
        session_id: 1,
        meeting_id: "meeting-1",
      },
    };

    await ctrl.toggleRecording();

    expect(mockInvoke).not.toHaveBeenCalledWith("start_transcription", expect.anything());
    expect(mockInvoke).not.toHaveBeenCalledWith("stop_transcription");
  });

  const signatureSnippet = {
    id: 1,
    trigger: "signature mail",
    expansion: "Cordialement, Damien",
    created_at: "",
  };

  it("applies a snippet matching the start of dictation and skips polish", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "list_snippets") return Promise.resolve([signatureSnippet]);
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      return defaultInvoke(cmd, args);
    });

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    ctrl.app.settings = { ...ctrl.app.settings, dictation_polish_enabled: true };

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "Signature mail, et à bientôt.",
      is_final: true,
    });
    await ctrl.toggleRecording();

    // The raw row written before finalization is replaced by the expansion (AC5).
    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("update_dictation_entry", expect.objectContaining({
        text: "Cordialement, Damien, et à bientôt.",
      }));
    });
    // A matched snippet never reaches the LLM (AC3).
    expect(mockInvoke).not.toHaveBeenCalledWith("polish_dictation", expect.anything());
  });

  it("finalization makes no snippet IPC call when no trigger matches", async () => {
    let transcriptionChannel: { onmessage: ((msg: unknown) => void) | null } | null = null;
    mockInvoke.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === "start_transcription") {
        transcriptionChannel = args?.channel as { onmessage: ((msg: unknown) => void) | null };
        return Promise.resolve(null);
      }
      return defaultInvoke(cmd, args);
    });
    const listSnippetCalls = () =>
      mockInvoke.mock.calls.filter(([cmd]) => cmd === "list_snippets").length;

    const ctrl = createTranscriptionController();
    await ctrl.mount();
    // The list is loaded once at mount (AC4: no extra IPC per dictation).
    expect(listSnippetCalls()).toBe(1);

    await ctrl.toggleRecording();
    simulateRecordingStarted(ctrl.app);
    (transcriptionChannel as { onmessage: ((msg: unknown) => void) | null } | null)?.onmessage?.({
      text: "hello world",
      is_final: true,
    });
    await ctrl.toggleRecording();

    await vi.waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith("add_dictation_entry", { text: "hello world" });
    });
    expect(listSnippetCalls()).toBe(1);
  });
});
