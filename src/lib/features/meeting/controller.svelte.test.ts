import { describe, it, expect, vi, beforeEach } from "vitest";
import type {
  MeetingTranscript,
  OllamaStatus,
  SummarizeProgress,
  TranscriptionCatalog,
  TranscriptionSegment,
} from "../../types";

// ── API mocks ────────────────────────────────────────────────────────

const mockGetOllamaStatus = vi.fn<() => Promise<OllamaStatus>>();
const mockGetTranscriptionCatalog = vi.fn<() => Promise<TranscriptionCatalog>>();
const mockStartMeetingRecording = vi.fn<
  (title: string, onSegment: (s: TranscriptionSegment) => void) => Promise<void>
>();
const mockResumeMeetingRecording = vi.fn<
  (id: string, onSegment: (s: TranscriptionSegment) => void) => Promise<void>
>();
const mockStopMeetingRecording = vi.fn<() => Promise<string>>();
const mockGetMeeting = vi.fn<(id: string) => Promise<MeetingTranscript>>();
const mockDeleteMeeting = vi.fn<(id: string) => Promise<void>>();
const mockSummarizeMeeting = vi.fn<
  (id: string, model: string, onProgress: (p: SummarizeProgress) => void) => Promise<void>
>();

vi.mock("../../api/meetings", () => ({
  startMeetingRecording: (...a: unknown[]) =>
    mockStartMeetingRecording(...(a as [string, (s: TranscriptionSegment) => void])),
  resumeMeetingRecording: (...a: unknown[]) =>
    mockResumeMeetingRecording(...(a as [string, (s: TranscriptionSegment) => void])),
  stopMeetingRecording: (...a: unknown[]) => mockStopMeetingRecording(...(a as [])),
  getMeeting: (...a: unknown[]) => mockGetMeeting(...(a as [string])),
  deleteMeeting: (...a: unknown[]) => mockDeleteMeeting(...(a as [string])),
  summarizeMeeting: (...a: unknown[]) =>
    mockSummarizeMeeting(
      ...(a as [string, string, (p: SummarizeProgress) => void]),
    ),
}));

vi.mock("../../api/ollama", () => ({
  getOllamaStatus: (...a: unknown[]) => mockGetOllamaStatus(...(a as [])),
}));

vi.mock("../../api/transcription", () => ({
  getTranscriptionCatalog: (...a: unknown[]) =>
    mockGetTranscriptionCatalog(...(a as [])),
}));

// ── App state mock ───────────────────────────────────────────────────

function createMockAppState() {
  return {
    currentView: "meeting" as string,
    currentMeetingId: null as string | null,
    isRecording: false,
    recordingMode: "idle" as string,
    settings: {
      theme: "dark" as const,
      auto_paste: false,
      paste_delay_ms: 100,
      ollama_url: "http://localhost:11434",
      ollama_model: "",
      debug_transcription: false,
      audio_device: null,
      transcription_engine_id: "kyutai",
      transcription_model_id: "stt-1b-en_fr",
      transcription_backend_id: "candle",
    },
    selectedDevice: "",
    openMeeting: vi.fn(),
    newMeeting: vi.fn(),
  };
}

let mockApp = createMockAppState();

vi.mock("../../stores/app.svelte", () => ({
  getAppState: () => mockApp,
}));

vi.mock("../transcription/catalog", () => ({
  toSelectedTranscriptionProfile: () => ({
    engine_id: "kyutai",
    engine_label: "Kyutai",
    model_id: "stt-1b-en_fr",
    model_label: "STT 1B",
    backend_id: "candle",
    backend_label: "Candle",
  }),
}));

const { createMeetingController, resetMeetingControllerForTest } = await import("./controller.svelte");

// ── Fixtures ─────────────────────────────────────────────────────────

function makeMeeting(overrides: Partial<MeetingTranscript> = {}): MeetingTranscript {
  return {
    id: "meet-1",
    title: "Standup",
    started_at: "2025-06-01T10:00:00Z",
    ended_at: "2025-06-01T10:30:00Z",
    duration_seconds: 1800,
    transcription_profile: {
      engine_id: "kyutai",
      engine_label: "Kyutai",
      model_id: "stt-1b-en_fr",
      model_label: "STT 1B",
      backend_id: "candle",
      backend_label: "Candle",
    },
    recording_sessions: [],
    segments: [],
    summary: null,
    summary_is_stale: false,
    summary_model: null,
    summary_generated_at: null,
    ...overrides,
  };
}

function makeOllamaStatus(overrides: Partial<OllamaStatus> = {}): OllamaStatus {
  return {
    available: true,
    base_url: "http://localhost:11434",
    models: [
      { id: "llama3", label: "Llama 3", can_summarize: true },
      { id: "codellama", label: "Code Llama", can_summarize: false },
    ],
    ...overrides,
  };
}

function makeCatalog(): TranscriptionCatalog {
  return {
    engines: [
      {
        id: "kyutai",
        label: "Kyutai",
        description: "Kyutai STT",
        models: [
          {
            id: "stt-1b-en_fr",
            label: "STT 1B",
            description: "1B model",
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
}

// ── Tests ────────────────────────────────────────────────────────────

describe("MeetingController", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    resetMeetingControllerForTest();
    mockApp = createMockAppState();
  });

  it("mount checks ollama and loads transcription catalog", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(mockGetOllamaStatus).toHaveBeenCalledOnce();
    expect(mockGetTranscriptionCatalog).toHaveBeenCalledOnce();
    expect(ctrl.ollamaAvailable).toBe(true);
    // Only models with can_summarize are kept
    expect(ctrl.summaryModels).toHaveLength(1);
    expect(ctrl.summaryModels[0].id).toBe("llama3");
  });

  it("startRecording sets recording state", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockStartMeetingRecording.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    ctrl.meetingTitle = "Sprint Review";
    await ctrl.startRecording();

    expect(mockStartMeetingRecording).toHaveBeenCalledOnce();
    expect(mockStartMeetingRecording.mock.calls[0][0]).toBe("Sprint Review");
    expect(ctrl.isRecordingMeeting).toBe(true);
    expect(mockApp.isRecording).toBe(true);
    expect(mockApp.recordingMode).toBe("meeting");
    // Title is cleared after start
    expect(ctrl.meetingTitle).toBe("");
  });

  it("stopRecording saves and loads meeting", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockStartMeetingRecording.mockResolvedValue(undefined);
    mockStopMeetingRecording.mockResolvedValue("meet-1");
    mockGetMeeting.mockResolvedValue(makeMeeting());

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.startRecording();
    await ctrl.stopRecording();

    expect(mockStopMeetingRecording).toHaveBeenCalledOnce();
    expect(mockGetMeeting).toHaveBeenCalledWith("meet-1");
    expect(ctrl.isRecordingMeeting).toBe(false);
    expect(mockApp.isRecording).toBe(false);
    expect(mockApp.recordingMode).toBe("idle");
    expect(mockApp.currentMeetingId).toBe("meet-1");
    expect(ctrl.meeting?.id).toBe("meet-1");
  });

  it("summarize streams progress text", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    // summarizeMeeting calls the onProgress callback with chunks
    mockSummarizeMeeting.mockImplementation(async (_id, _model, onProgress) => {
      onProgress({ text: "Summary ", done: false });
      onProgress({ text: "complete.", done: false });
      // Final chunk with done=true triggers meeting reload
      mockGetMeeting.mockResolvedValue(
        makeMeeting({ summary: "Summary complete.", summary_model: "llama3" }),
      );
      onProgress({ text: "", done: true });
    });

    const ctrl = createMeetingController();
    await ctrl.mount();

    // Set up controller state as if a meeting is loaded
    mockGetMeeting.mockResolvedValue(makeMeeting());
    await ctrl.onMeetingSelectionChange("meet-1");
    ctrl.selectedModel = "llama3";

    await ctrl.summarizeMeeting();

    expect(mockSummarizeMeeting).toHaveBeenCalledWith("meet-1", "llama3", expect.any(Function));
    expect(ctrl.summaryStream).toBe("Summary complete.");
  });

  it("summarize without selected model is noop", async () => {
    mockGetOllamaStatus.mockResolvedValue(
      makeOllamaStatus({ available: false, models: [] }),
    );
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const ctrl = createMeetingController();
    await ctrl.mount();

    // No model selected, no meeting — should bail immediately
    await ctrl.summarizeMeeting();

    expect(mockSummarizeMeeting).not.toHaveBeenCalled();
    expect(ctrl.isSummarizing).toBe(false);
  });

  it("deleteMeeting clears state and navigates to history", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockDeleteMeeting.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.deleteMeeting();

    expect(mockDeleteMeeting).toHaveBeenCalledWith("meet-1");
    expect(ctrl.meeting).toBeNull();
    expect(mockApp.currentMeetingId).toBeNull();
    expect(mockApp.currentView).toBe("meeting-history");
  });

  it("canResumeRecording is true when meeting loaded and not recording", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    // Has meeting, not recording, not loading, not summarizing
    expect(ctrl.canResumeRecording).toBe(true);
  });

  it("resumeRecording reuses existing meeting", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockResumeMeetingRecording.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.resumeRecording();

    expect(mockResumeMeetingRecording).toHaveBeenCalledOnce();
    expect(mockResumeMeetingRecording.mock.calls[0][0]).toBe("meet-1");
    expect(ctrl.isRecordingMeeting).toBe(true);
    expect(mockApp.isRecording).toBe(true);
    expect(mockApp.recordingMode).toBe("meeting");
  });

  it("syncSelectedModel picks preferred, then settings, then first available", async () => {
    mockGetOllamaStatus.mockResolvedValue(makeOllamaStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const ctrl = createMeetingController();
    await ctrl.mount();

    // After mount with ollama available, selectedModel should be llama3
    // (first can_summarize model since no preference set)
    expect(ctrl.selectedModel).toBe("llama3");

    // If settings has a preferred model and it exists in the list
    mockApp.settings.ollama_model = "llama3";
    await ctrl.checkOllama();
    expect(ctrl.selectedModel).toBe("llama3");
  });
});
