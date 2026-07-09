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

import { createTranscriptionController, resetTranscriptionControllerForTest } from "./controller.svelte";
import {
  startTranscriptionModelDownload,
  startTranscriptionModelLoad,
} from "./runtime";
import { getAppState } from "../../stores/app.svelte";
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
  const selection = {
    engine_id: "kyutai",
    model_id: "stt-1b-en_fr",
    backend_id: "candle",
  };

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
        return Promise.resolve(null);
      case "delete_dictation_entry":
        return Promise.resolve(null);
      case "clear_dictation_history":
        return Promise.resolve(null);
      case "paste_text":
        return Promise.resolve(null);
      case "polish_dictation":
        return Promise.resolve({ text: args?.text ?? "", skipped: true, warning: null });
      case "load_model":
        return Promise.resolve(null);
      case "download_model":
        return Promise.resolve(null);
      case "save_settings":
        return Promise.resolve(null);
      default:
        return Promise.resolve(null);
    }
  }

  beforeEach(() => {
    vi.clearAllMocks();
    resetTranscriptionControllerForTest();
    mockInvoke.mockImplementation(defaultInvoke);
    mockListen.mockResolvedValue(mockUnlisten);

    // Reset shared singleton app state between tests
    const app = getAppState();
    app.currentMeetingId = null;
    app.machineState = { state: "idle" };
    app.transcriptionRuntimePhase = "download_required";
    app.downloadFile = "";
    app.downloadCompletedFiles = 0;
    app.downloadTotalFiles = 0;
    app.selectedDevice = "";
    app.settings = {
      theme: "dark",
      locale: "",
      auto_paste: false,
      paste_delay_ms: 100,
      ollama_url: "http://localhost:11434",
      ollama_model: "",
      debug_transcription: false,
      audio_device: null,
      clamshell_audio_device: null,
      transcription_engine_id: "kyutai",
      transcription_model_id: "stt-1b-en_fr",
      transcription_backend_id: "candle",
      vad_enabled: true,
      filler_removal: true,
      stutter_collapse: false,
      dictionary_correction: true,
      capture_system_audio: true,
      calendar_integration_enabled: false,
      calendar_selected_ids: [],
      calendar_reminder_minutes: 2,
      model_unload_timeout_minutes: 0,
      meeting_autostop_enabled: true,
      meeting_autostop_minutes: 10,
      meeting_max_duration_minutes: 240,
      dictation_polish_enabled: false,
      dictation_polish_template_id: "email",
      dictation_polish_templates: [
        { id: "email", label: "Professional email", prompt: "Rewrite as email." },
        { id: "bullets", label: "Bullet points", prompt: "Use bullets." },
        { id: "no_fillers", label: "Remove fillers", prompt: "Remove fillers." },
      ],
    };

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
  function simulateRecordingStarted(app: ReturnType<typeof getAppState>) {
    app.machineState = { state: "recording_dictation", data: { profile: { engine_id: "kyutai", engine_label: "Kyutai", model_id: "stt-1b-en_fr", model_label: "STT 1B", backend_id: "candle", backend_label: "Candle" }, session_id: 1 } };
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

    resolveStart!();
    await first;
    await second;

    const startCalls = mockInvoke.mock.calls.filter((call) => call[0] === "start_transcription");
    expect(startCalls).toHaveLength(1);
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

});
