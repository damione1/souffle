import { describe, it, expect, vi, beforeEach } from "vitest";
import type {
  MeetingCalendarContext,
  MeetingIdle,
  MeetingTranscript,
  SummaryProvidersStatus,
  SummarizeProgress,
  TranscriptionCatalog,
  TranscriptionSegment,
} from "../../types";

// ── API mocks ────────────────────────────────────────────────────────

const mockGetSummaryProvidersStatus = vi.fn<() => Promise<SummaryProvidersStatus>>();
const mockGetTranscriptionCatalog = vi.fn<() => Promise<TranscriptionCatalog>>();
const mockStartMeetingRecording = vi.fn<
  (
    title: string,
    calendar: MeetingCalendarContext | null,
    onSegment: (s: TranscriptionSegment) => void,
  ) => Promise<void>
>();
const mockResumeMeetingRecording = vi.fn<
  (id: string, onSegment: (s: TranscriptionSegment) => void) => Promise<void>
>();
const mockStopMeetingRecording = vi.fn<() => Promise<string>>();
const mockTakeSleepPausedMeeting = vi.fn<() => Promise<string | null>>();
const mockGetMeeting = vi.fn<(id: string) => Promise<MeetingTranscript>>();
const mockDeleteMeeting = vi.fn<(id: string) => Promise<void>>();
const mockRenameMeeting = vi.fn<(id: string, title: string) => Promise<void>>();
const mockSaveMeetingNotes = vi.fn<(id: string, notes: string | null) => Promise<void>>();
const mockSummarizeMeeting = vi.fn<
  (id: string, model: string, onProgress: (p: SummarizeProgress) => void) => Promise<void>
>();
const mockExportMeetingFilename = vi.fn<
  (id: string, format: import("../../types").ExportFormat) => Promise<string>
>();
const mockExportMeetingToFile = vi.fn<
  (id: string, format: import("../../types").ExportFormat, path: string) => Promise<void>
>();
const mockShowSaveDialog = vi.fn<(opts: unknown) => Promise<string | null>>();

vi.mock("../../api/meetings", () => ({
  startMeetingRecording: (...a: unknown[]) =>
    mockStartMeetingRecording(
      ...(a as [string, MeetingCalendarContext | null, (s: TranscriptionSegment) => void]),
    ),
  resumeMeetingRecording: (...a: unknown[]) =>
    mockResumeMeetingRecording(...(a as [string, (s: TranscriptionSegment) => void])),
  stopMeetingRecording: (...a: unknown[]) => mockStopMeetingRecording(...(a as [])),
  takeSleepPausedMeeting: (...a: unknown[]) => mockTakeSleepPausedMeeting(...(a as [])),
  getMeeting: (...a: unknown[]) => mockGetMeeting(...(a as [string])),
  deleteMeeting: (...a: unknown[]) => mockDeleteMeeting(...(a as [string])),
  renameMeeting: (...a: unknown[]) => mockRenameMeeting(...(a as [string, string])),
  saveMeetingNotes: (...a: unknown[]) =>
    mockSaveMeetingNotes(...(a as [string, string | null])),
  saveEditedTranscript: vi.fn(),
  summarizeMeeting: (...a: unknown[]) =>
    mockSummarizeMeeting(
      ...(a as [string, string, (p: SummarizeProgress) => void]),
    ),
  exportMeetingFilename: (...a: unknown[]) =>
    mockExportMeetingFilename(...(a as [string, import("../../types").ExportFormat])),
  exportMeetingToFile: (...a: unknown[]) =>
    mockExportMeetingToFile(...(a as [string, import("../../types").ExportFormat, string])),
}));

vi.mock("@tauri-apps/plugin-dialog", () => ({
  save: (...a: unknown[]) => mockShowSaveDialog(...(a as [unknown])),
}));

vi.mock("../../api/summary", () => ({
  getSummaryProvidersStatus: (...a: unknown[]) => mockGetSummaryProvidersStatus(...(a as [])),
}));

vi.mock("../../api/transcription", () => ({
  getTranscriptionCatalog: (...a: unknown[]) =>
    mockGetTranscriptionCatalog(...(a as [])),
}));

// ── App state mock ───────────────────────────────────────────────────

function createMockAppState() {
  return {
    currentMeetingId: null as string | null,
    machineState: { state: "idle" } as import("../../types").AppStateMachine,
    isRecording: false,
    recordingMode: "idle" as string,
    transcriptionRuntimePhase: "ready" as string,
    settings: {
      theme: "dark" as const,
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

const {
  createMeetingController,
  resetMeetingControllerForTest,
  notifyMeetingIdle,
  notifySystemWokeUp,
} = await import("./controller.svelte");

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
    structured_summary: null,
    edited_transcript: null,
    notes: null,
    calendar_event_id: null,
    participants: [],
    ...overrides,
  };
}

function makeSummaryProvidersStatus(
  overrides: Partial<SummaryProvidersStatus> = {},
): SummaryProvidersStatus {
  return {
    ollama_url: "http://localhost:11434",
    ollama_available: true,
    apple_intelligence_available: false,
    apple_intelligence_is_stub: true,
    models: [
      { id: "llama3", label: "Llama 3", provider: "ollama", can_summarize: true },
      { id: "codellama", label: "Code Llama", provider: "ollama", can_summarize: false },
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
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(mockGetSummaryProvidersStatus).toHaveBeenCalledOnce();
    expect(mockGetTranscriptionCatalog).toHaveBeenCalledOnce();
    expect(ctrl.ollamaAvailable).toBe(true);
    // Only models with can_summarize are kept
    expect(ctrl.summaryModels).toHaveLength(1);
    expect(ctrl.summaryModels[0].id).toBe("llama3");
  });

  it("startRecording starts with a dated default title", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockStartMeetingRecording.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.startRecording();

    expect(mockStartMeetingRecording).toHaveBeenCalledOnce();
    expect(mockStartMeetingRecording.mock.calls[0][0]).toMatch(/^Meeting /);
    expect(mockStartMeetingRecording.mock.calls[0][1]).toBeNull();
    expect(ctrl.meeting?.title).toMatch(/^Meeting /);
  });

  it("startRecording from a calendar event uses the event title and carries participants", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockStartMeetingRecording.mockResolvedValue(undefined);

    const calendar = {
      event_id: "evt-1",
      participants: [
        { name: "Alice", email: "alice@corp.com", is_organizer: true, is_current_user: false },
      ],
      description: null,
    };

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.startRecording({ title: "Sprint Planning", calendar });

    expect(mockStartMeetingRecording).toHaveBeenCalledOnce();
    expect(mockStartMeetingRecording.mock.calls[0][0]).toBe("Sprint Planning");
    expect(mockStartMeetingRecording.mock.calls[0][1]).toEqual(calendar);
    expect(ctrl.meeting?.title).toBe("Sprint Planning");
    expect(ctrl.meeting?.calendar_event_id).toBe("evt-1");
    expect(ctrl.meeting?.participants).toEqual(calendar.participants);
  });

  it("stopRecording saves and loads meeting", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
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
    expect(mockApp.currentMeetingId).toBe("meet-1");
    expect(ctrl.meeting?.id).toBe("meet-1");
  });

  it("summarize streams progress text", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
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
    mockGetSummaryProvidersStatus.mockResolvedValue(
      makeSummaryProvidersStatus({ ollama_available: false, models: [] }),
    );
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const ctrl = createMeetingController();
    await ctrl.mount();

    // No model selected, no meeting — should bail immediately
    await ctrl.summarizeMeeting();

    expect(mockSummarizeMeeting).not.toHaveBeenCalled();
    expect(ctrl.isSummarizing).toBe(false);
  });

  it("deleteMeeting clears state and returns to the list", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
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
  });

  it("exportMeeting looks up the filename, opens the save dialog, and writes the file", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockExportMeetingFilename.mockResolvedValue("2026-07-09-standup.md");
    mockShowSaveDialog.mockResolvedValue("/Users/damien/Downloads/2026-07-09-standup.md");
    mockExportMeetingToFile.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeeting("markdown");

    expect(mockExportMeetingFilename).toHaveBeenCalledWith("meet-1", "markdown");
    expect(mockShowSaveDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        defaultPath: "2026-07-09-standup.md",
        filters: [{ name: "MD", extensions: ["md"] }],
      }),
    );
    expect(mockExportMeetingToFile).toHaveBeenCalledWith(
      "meet-1",
      "markdown",
      "/Users/damien/Downloads/2026-07-09-standup.md",
    );
    expect(ctrl.isExporting).toBe(false);
  });

  it("exportMeeting does not write a file when the save dialog is cancelled", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockExportMeetingFilename.mockResolvedValue("2026-07-09-standup.srt");
    mockShowSaveDialog.mockResolvedValue(null);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeeting("srt");

    expect(mockShowSaveDialog).toHaveBeenCalledOnce();
    expect(mockExportMeetingToFile).not.toHaveBeenCalled();
  });

  it("exportMeeting surfaces backend errors via statusMessage", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockExportMeetingFilename.mockRejectedValue(new Error("meeting not found"));

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeeting("json");

    expect(ctrl.statusMessage).toContain("meeting not found");
    expect(mockShowSaveDialog).not.toHaveBeenCalled();
    expect(ctrl.isExporting).toBe(false);
  });

  it("exportMeeting is a no-op while the meeting is recording", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockStartMeetingRecording.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.startRecording();
    mockApp.machineState = {
      state: "recording_meeting",
      data: {
        profile: makeMeeting().transcription_profile,
        session_id: 1,
        meeting_id: "live-1",
      },
    } as import("../../types").AppStateMachine;

    await ctrl.exportMeeting("vtt");

    expect(mockExportMeetingFilename).not.toHaveBeenCalled();
    expect(mockShowSaveDialog).not.toHaveBeenCalled();
  });

  it("notes autosave debounces and targets the live accumulator id", async () => {
    vi.useFakeTimers();
    try {
      mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
      mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
      mockStartMeetingRecording.mockResolvedValue(undefined);
      mockSaveMeetingNotes.mockResolvedValue(undefined);

      const ctrl = createMeetingController();
      await ctrl.mount();
      await ctrl.startRecording();
      mockApp.machineState = {
        state: "recording_meeting",
        data: {
          profile: makeMeeting().transcription_profile,
          session_id: 1,
          meeting_id: "live-1",
        },
      } as import("../../types").AppStateMachine;

      ctrl.onNotesChange("first");
      ctrl.onNotesChange("first draft");
      expect(mockSaveMeetingNotes).not.toHaveBeenCalled();

      await vi.advanceTimersByTimeAsync(900);
      expect(mockSaveMeetingNotes).toHaveBeenCalledTimes(1);
      expect(mockSaveMeetingNotes).toHaveBeenCalledWith("live-1", "first draft");
      expect(ctrl.notesSaveState).toBe("saved");
    } finally {
      vi.useRealTimers();
    }
  });

  it("canResumeRecording is true when meeting loaded and not recording", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    // Has meeting, not recording, not loading, not summarizing
    expect(ctrl.canResumeRecording).toBe(true);
  });

  it("resumeRecording reuses existing meeting", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockResumeMeetingRecording.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.resumeRecording();

    expect(mockResumeMeetingRecording).toHaveBeenCalledOnce();
    expect(mockResumeMeetingRecording.mock.calls[0][0]).toBe("meet-1");
  });

  it("syncSelectedModel picks preferred, then ollama settings, then first available", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.selectedModel).toBe("llama3");

    mockApp.settings.ollama_model = "llama3";
    await ctrl.refreshSummaryProviders();
    expect(ctrl.selectedModel).toBe("llama3");
  });

  it("syncSelectedModel prefers saved ollama_model over apple when both available", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(
      makeSummaryProvidersStatus({
        apple_intelligence_available: true,
        models: [
          {
            id: "apple-intelligence",
            label: "Apple Intelligence",
            provider: "apple_intelligence",
            can_summarize: true,
          },
          { id: "llama3", label: "Llama 3", provider: "ollama", can_summarize: true },
        ],
      }),
    );
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockApp.settings.ollama_model = "llama3";

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.selectedModel).toBe("llama3");
  });

  describe("notifyMeetingIdle", () => {
    function setRecordingMeeting() {
      mockApp.machineState = {
        state: "recording_meeting",
        data: {
          profile: makeMeeting().transcription_profile,
          session_id: 1,
          meeting_id: "live-1",
        },
      } as import("../../types").AppStateMachine;
    }

    function idle(overrides: Partial<MeetingIdle> = {}): MeetingIdle {
      return { reason: "silence", idle_seconds: 60, threshold_seconds: 600, ...overrides };
    }

    async function mountedController() {
      mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
      mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
      const ctrl = createMeetingController();
      await ctrl.mount();
      return ctrl;
    }

    it("is ignored entirely when not recording a meeting", async () => {
      const ctrl = await mountedController();
      notifyMeetingIdle(idle());
      expect(ctrl.idleSignal).toBeNull();
      expect(mockStopMeetingRecording).not.toHaveBeenCalled();
    });

    it("max_duration sets a status message and stops immediately", async () => {
      mockStopMeetingRecording.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      const ctrl = await mountedController();
      setRecordingMeeting();

      notifyMeetingIdle(idle({ reason: "max_duration", idle_seconds: 14400, threshold_seconds: 14400 }));
      await vi.waitFor(() => expect(mockStopMeetingRecording).toHaveBeenCalledOnce());

      expect(ctrl.statusMessage).toMatch(/maximum meeting duration/i);
    });

    it("max_duration does not double-stop while already stopping", async () => {
      mockStopMeetingRecording.mockImplementation(() => new Promise(() => {})); // never resolves
      const ctrl = await mountedController();
      setRecordingMeeting();

      notifyMeetingIdle(idle({ reason: "max_duration" }));
      notifyMeetingIdle(idle({ reason: "max_duration" }));
      await Promise.resolve();

      expect(mockStopMeetingRecording).toHaveBeenCalledOnce();
    });

    it("silence sets idleSignal without stopping before the grace period", async () => {
      const ctrl = await mountedController();
      setRecordingMeeting();

      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 601, threshold_seconds: 600 }));

      expect(ctrl.idleSignal).toEqual(idle({ reason: "silence", idle_seconds: 601, threshold_seconds: 600 }));
      expect(mockStopMeetingRecording).not.toHaveBeenCalled();
    });

    it("silence auto-stops once idle_seconds reaches threshold + 120s grace", async () => {
      mockStopMeetingRecording.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      const ctrl = await mountedController();
      setRecordingMeeting();

      // Still under the grace window: banner shows, no stop yet.
      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 719, threshold_seconds: 600 }));
      expect(mockStopMeetingRecording).not.toHaveBeenCalled();
      expect(ctrl.idleSignal).not.toBeNull();

      // Crosses threshold + 120s: auto-stop fires.
      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 720, threshold_seconds: 600 }));
      await vi.waitFor(() => expect(mockStopMeetingRecording).toHaveBeenCalledOnce());
    });

    it("dismissIdle suppresses further silence banners until a segment re-arms it", async () => {
      const ctrl = await mountedController();
      setRecordingMeeting();

      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 601, threshold_seconds: 600 }));
      expect(ctrl.idleSignal).not.toBeNull();

      ctrl.dismissIdle();
      expect(ctrl.idleSignal).toBeNull();

      // Still silent: dismissed state suppresses the banner from reappearing.
      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 631, threshold_seconds: 600 }));
      expect(ctrl.idleSignal).toBeNull();
      expect(mockStopMeetingRecording).not.toHaveBeenCalled();
    });

    it("a new transcript segment clears idleSignal and re-arms after dismissal", async () => {
      let onSegmentCallback: ((segment: TranscriptionSegment) => void) | undefined;
      mockStartMeetingRecording.mockImplementation(async (_title, _calendar, onSegment) => {
        onSegmentCallback = onSegment;
      });

      const ctrl = await mountedController();
      await ctrl.startRecording();
      setRecordingMeeting();

      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 601, threshold_seconds: 600 }));
      expect(ctrl.idleSignal).not.toBeNull();
      ctrl.dismissIdle();

      // Speech resumes: a final segment with text clears the banner and re-arms.
      onSegmentCallback?.({
        text: "we're back",
        start_time: 0,
        end_time: 1,
        is_final: true,
        language: null,
        confidence: null,
        speaker: null,
      });

      notifyMeetingIdle(idle({ reason: "silence", idle_seconds: 601, threshold_seconds: 600 }));
      expect(ctrl.idleSignal).not.toBeNull();
    });
  });

  describe("notifySystemWokeUp", () => {
    it("loads and auto-resumes the meeting sleep paused, when one exists", async () => {
      mockTakeSleepPausedMeeting.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockResumeMeetingRecording.mockResolvedValue(undefined);

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockResumeMeetingRecording).toHaveBeenCalledOnce());

      expect(mockTakeSleepPausedMeeting).toHaveBeenCalledOnce();
      expect(mockGetMeeting).toHaveBeenCalledWith("meet-1");
      expect(mockResumeMeetingRecording.mock.calls[0][0]).toBe("meet-1");
      expect(ctrl.statusMessage).toMatch(/resumed after sleep/i);
    });

    it("does nothing when no meeting was paused by sleep", async () => {
      mockTakeSleepPausedMeeting.mockResolvedValue(null);

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockTakeSleepPausedMeeting).toHaveBeenCalledOnce());

      expect(mockGetMeeting).not.toHaveBeenCalled();
      expect(mockResumeMeetingRecording).not.toHaveBeenCalled();
      expect(ctrl.meeting).toBeNull();
    });

    it("leaves the meeting loaded (not resumed) and surfaces the error when resume fails", async () => {
      mockTakeSleepPausedMeeting.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockResumeMeetingRecording.mockRejectedValue(new Error("model unload failed"));

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockResumeMeetingRecording).toHaveBeenCalledOnce());

      expect(ctrl.meeting?.id).toBe("meet-1");
      expect(ctrl.statusMessage).toMatch(/model unload failed/i);
    });
  });
});
