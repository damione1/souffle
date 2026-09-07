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
import { mockSettings } from "../../test-helpers/fixtures";

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
const mockPeekSleepPausedMeeting = vi.fn<() => Promise<string | null>>();
const mockClearSleepPausedMeeting = vi.fn<() => Promise<void>>();
const mockGetMeeting = vi.fn<(id: string) => Promise<MeetingTranscript>>();
const mockDeleteMeeting = vi.fn<(id: string) => Promise<void>>();
const mockRenameMeeting = vi.fn<(id: string, title: string) => Promise<void>>();
const mockSaveMeetingNotes = vi.fn<(id: string, notes: string | null) => Promise<void>>();
const mockSummarizeMeeting = vi.fn<
  (
    id: string,
    model: string,
    templateId: string | null,
    onProgress: (p: SummarizeProgress) => void,
  ) => Promise<void>
>();
const mockSaveMeetingExport = vi.fn<
  (id: string, format: import("../../types").ExportFormat) => Promise<void>
>();
const mockSaveMeetingAudioExport = vi.fn<(id: string) => Promise<void>>();
const mockGetMeetingAudio = vi.fn<(id: string) => Promise<import("../../types").MeetingAudioSession[]>>();
const mockApplyLiveParagraphEdit = vi.fn<
  (meetingId: string, segmentIndices: number[], newText: string) => Promise<void>
>();

vi.mock("../../api/meetings", () => ({
  startMeetingRecording: (...a: unknown[]) =>
    mockStartMeetingRecording(
      ...(a as [string, MeetingCalendarContext | null, (s: TranscriptionSegment) => void]),
    ),
  resumeMeetingRecording: (...a: unknown[]) =>
    mockResumeMeetingRecording(...(a as [string, (s: TranscriptionSegment) => void])),
  stopMeetingRecording: (...a: unknown[]) => mockStopMeetingRecording(...(a as [])),
  peekSleepPausedMeeting: (...a: unknown[]) => mockPeekSleepPausedMeeting(...(a as [])),
  clearSleepPausedMeeting: (...a: unknown[]) => mockClearSleepPausedMeeting(...(a as [])),
  getMeeting: (...a: unknown[]) => mockGetMeeting(...(a as [string])),
  deleteMeeting: (...a: unknown[]) => mockDeleteMeeting(...(a as [string])),
  renameMeeting: (...a: unknown[]) => mockRenameMeeting(...(a as [string, string])),
  saveMeetingNotes: (...a: unknown[]) =>
    mockSaveMeetingNotes(...(a as [string, string | null])),
  saveEditedTranscript: vi.fn(),
  summarizeMeeting: (...a: unknown[]) =>
    mockSummarizeMeeting(
      ...(a as [string, string, string | null, (p: SummarizeProgress) => void]),
    ),
  saveMeetingExport: (...a: unknown[]) =>
    mockSaveMeetingExport(...(a as [string, import("../../types").ExportFormat])),
  saveMeetingAudioExport: (...a: unknown[]) =>
    mockSaveMeetingAudioExport(...(a as [string])),
  getMeetingAudio: (...a: unknown[]) => mockGetMeetingAudio(...(a as [string])),
  applyLiveParagraphEdit: (...a: unknown[]) =>
    mockApplyLiveParagraphEdit(...(a as [string, number[], string])),
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
    settings: { ...mockSettings },
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
  notifyMeetingFinalized,
  notifyStateChanged,
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
    apple_intelligence_unavailable_reason: "stub",
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
    mockGetMeetingAudio.mockResolvedValue([]);
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

  function makeBothProvidersStatus(): SummaryProvidersStatus {
    return makeSummaryProvidersStatus({
      apple_intelligence_available: true,
      apple_intelligence_is_stub: false,
      apple_intelligence_unavailable_reason: null,
      models: [
        {
          id: "apple-intelligence",
          label: "Apple Intelligence",
          provider: "apple_intelligence",
          can_summarize: true,
        },
        { id: "qwen2.5:7b", label: "qwen2.5:7b", provider: "ollama", can_summarize: true },
        { id: "codellama", label: "Code Llama", provider: "ollama", can_summarize: false },
      ],
    });
  }

  it("auto keeps both providers and defaults to Apple Intelligence", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeBothProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockApp.settings.summary_provider = "auto";
    mockApp.settings.ollama_model = "qwen2.5:7b";

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.summaryModels.map((model) => model.id)).toEqual([
      "apple-intelligence",
      "qwen2.5:7b",
    ]);
    expect(ctrl.selectedModel).toBe("apple-intelligence");
  });

  it("an explicit Ollama choice hides Apple Intelligence from the picker", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeBothProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockApp.settings.summary_provider = "ollama";
    mockApp.settings.ollama_model = "qwen2.5:7b";

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.summaryModels.map((model) => model.id)).toEqual(["qwen2.5:7b"]);
    expect(ctrl.selectedModel).toBe("qwen2.5:7b");
  });

  it("an explicit Apple Intelligence choice hides Ollama from the picker", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeBothProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockApp.settings.summary_provider = "apple_intelligence";

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.summaryModels.map((model) => model.id)).toEqual(["apple-intelligence"]);
    expect(ctrl.selectedModel).toBe("apple-intelligence");
  });

  it("explicit Ollama with only Apple available offers no model", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(
      makeSummaryProvidersStatus({
        apple_intelligence_available: true,
        apple_intelligence_is_stub: false,
        apple_intelligence_unavailable_reason: null,
        ollama_available: false,
        models: [
          {
            id: "apple-intelligence",
            label: "Apple Intelligence",
            provider: "apple_intelligence",
            can_summarize: true,
          },
        ],
      }),
    );
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockApp.settings.summary_provider = "ollama";

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.summaryModels).toEqual([]);
    expect(ctrl.selectedModel).toBe("");
    expect(ctrl.summaryAvailable).toBe(false);
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

    // summarizeMeeting calls the onProgress callback with chunks, including
    // non-text stage markers (map/combine/extract) that must not corrupt the
    // accumulated stream text.
    mockSummarizeMeeting.mockImplementation(async (_id, _model, _templateId, onProgress) => {
      onProgress({ text: "", done: false, stage: "map", current: 1, total: 2 });
      onProgress({ text: "", done: false, stage: "combine", current: null, total: null });
      onProgress({ text: "Summary ", done: false, stage: "final", current: null, total: null });
      onProgress({ text: "complete.", done: false, stage: "final", current: null, total: null });
      // Final chunk with done=true triggers meeting reload
      mockGetMeeting.mockResolvedValue(
        makeMeeting({ summary: "Summary complete.", summary_model: "llama3" }),
      );
      onProgress({ text: "", done: true, stage: "final", current: null, total: null });
    });

    const ctrl = createMeetingController();
    await ctrl.mount();

    // Set up controller state as if a meeting is loaded
    mockGetMeeting.mockResolvedValue(makeMeeting());
    await ctrl.onMeetingSelectionChange("meet-1");
    ctrl.selectedModel = "llama3";

    await ctrl.summarizeMeeting();

    expect(mockSummarizeMeeting).toHaveBeenCalledWith(
      "meet-1",
      "llama3",
      "default",
      expect.any(Function),
    );
    expect(ctrl.summaryStream).toBe("Summary complete.");
    // Stage resets once generation finishes.
    expect(ctrl.summaryStage).toBeNull();
  });

  it("summarize passes the user-picked template id, defaulting to settings", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockSummarizeMeeting.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    mockGetMeeting.mockResolvedValue(makeMeeting());
    await ctrl.onMeetingSelectionChange("meet-1");
    ctrl.selectedModel = "llama3";

    // Preselected to the default template from settings.
    expect(ctrl.selectedTemplateId).toBe("default");

    ctrl.selectedTemplateId = "brief_overview";
    await ctrl.summarizeMeeting();
    expect(mockSummarizeMeeting).toHaveBeenLastCalledWith(
      "meet-1",
      "llama3",
      "brief_overview",
      expect.any(Function),
    );

    // A stale pick (template no longer exists) falls back to the default.
    ctrl.selectedTemplateId = "deleted-template";
    await ctrl.summarizeMeeting();
    expect(mockSummarizeMeeting).toHaveBeenLastCalledWith(
      "meet-1",
      "llama3",
      "default",
      expect.any(Function),
    );
  });

  it("summarize reports live stage progress while running", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());

    const observedStages: Array<{ stage: string | null; current: number | null; total: number | null }> = [];
    mockSummarizeMeeting.mockImplementation(async (_id, _model, _templateId, onProgress) => {
      onProgress({ text: "", done: false, stage: "map", current: 1, total: 3 });
      observedStages.push({ stage: ctrl.summaryStage, current: ctrl.summaryStageCurrent, total: ctrl.summaryStageTotal });
      onProgress({ text: "", done: false, stage: "combine", current: null, total: 3 });
      observedStages.push({ stage: ctrl.summaryStage, current: ctrl.summaryStageCurrent, total: ctrl.summaryStageTotal });
      onProgress({ text: "", done: false, stage: "extract", current: null, total: null });
      observedStages.push({ stage: ctrl.summaryStage, current: ctrl.summaryStageCurrent, total: ctrl.summaryStageTotal });
      mockGetMeeting.mockResolvedValue(makeMeeting({ summary: "done", summary_model: "llama3" }));
    });

    const ctrl = createMeetingController();
    await ctrl.mount();
    mockGetMeeting.mockResolvedValue(makeMeeting());
    await ctrl.onMeetingSelectionChange("meet-1");
    ctrl.selectedModel = "llama3";

    await ctrl.summarizeMeeting();

    expect(observedStages).toEqual([
      { stage: "map", current: 1, total: 3 },
      { stage: "combine", current: null, total: 3 },
      { stage: "extract", current: null, total: null },
    ]);
    expect(ctrl.summaryStage).toBeNull();
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

  it("exportMeeting asks the backend to show the save dialog and write the file", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockSaveMeetingExport.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeeting("markdown");

    expect(mockSaveMeetingExport).toHaveBeenCalledWith("meet-1", "markdown");
    expect(ctrl.isExporting).toBe(false);
  });

  it("exportMeeting surfaces backend errors via statusMessage", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockSaveMeetingExport.mockRejectedValue(new Error("meeting not found"));

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeeting("json");

    expect(ctrl.statusMessage).toContain("meeting not found");
    expect(ctrl.isExporting).toBe(false);
  });

  describe("applyLiveParagraphEdit", () => {
    async function recordOneParagraph() {
      mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
      mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
      let emit: (s: TranscriptionSegment) => void = () => {};
      mockStartMeetingRecording.mockImplementation(async (_t, _c, onSegment) => {
        emit = onSegment;
      });

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

      // Two turns far enough apart that the first one freezes into `committed`.
      emit(liveSeg("hello", 0));
      emit(liveSeg("there.", 0.3));
      emit(liveSeg("much", 30));
      emit(liveSeg("later.", 30.3));
      return { ctrl, emit };
    }

    function liveSeg(text: string, start: number): TranscriptionSegment {
      return {
        text,
        start_time: start,
        end_time: start + 0.2,
        is_final: true,
        language: null,
        confidence: null,
        speaker: null,
      };
    }

    it("rolls the paragraph and its segments back when the backend rejects", async () => {
      const { ctrl } = await recordOneParagraph();
      const paragraph = ctrl.liveTranscript.committed[0];
      mockApplyLiveParagraphEdit.mockRejectedValue(new Error("db is locked"));

      await ctrl.applyLiveParagraphEdit(paragraph.id, "hello world");

      expect(ctrl.liveTranscript.committed[0].text).toBe("hello there.");
      expect(ctrl.liveMeetingSegments.map((s) => s.text)).toEqual([
        "hello",
        "there.",
        "much",
        "later.",
      ]);
      expect(ctrl.statusMessage).toContain("db is locked");
    });

    it("does not roll back into the buffers of the next recording", async () => {
      const { ctrl } = await recordOneParagraph();
      const paragraph = ctrl.liveTranscript.committed[0];

      let reject: (e: Error) => void = () => {};
      mockApplyLiveParagraphEdit.mockImplementation(
        () => new Promise<void>((_resolve, r) => { reject = (e) => r(e); }),
      );

      const pending = ctrl.applyLiveParagraphEdit(paragraph.id, "hello world");
      // The meeting stops and a fresh one starts: paragraph ids restart at 0
      // and liveMeetingSegments is a new, empty array.
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockApp.currentMeetingId = "live-1";
      notifyMeetingFinalized("live-1");
      reject(new Error("db is locked"));
      await pending;

      expect(ctrl.liveMeetingSegments).toEqual([]);
      expect(ctrl.liveTranscript.committed).toEqual([]);
      expect(ctrl.statusMessage).not.toContain("db is locked");
    });
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

    expect(mockSaveMeetingExport).not.toHaveBeenCalled();
  });

  it("exportMeetingAudio asks the backend to show the save dialog and copy the file", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());
    mockGetMeetingAudio.mockResolvedValue([
      { session_index: 0, path: "/recordings/meet-1/0.ogg", duration_seconds: null },
    ]);
    mockSaveMeetingAudioExport.mockResolvedValue(undefined);

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeetingAudio();

    expect(mockSaveMeetingAudioExport).toHaveBeenCalledWith("meet-1");
    expect(ctrl.isExporting).toBe(false);
  });

  it("exportMeetingAudio is a no-op when the meeting has no recorded audio", async () => {
    mockGetSummaryProvidersStatus.mockResolvedValue(makeSummaryProvidersStatus());
    mockGetTranscriptionCatalog.mockResolvedValue(makeCatalog());
    mockGetMeeting.mockResolvedValue(makeMeeting());

    const ctrl = createMeetingController();
    await ctrl.mount();
    await ctrl.onMeetingSelectionChange("meet-1");

    await ctrl.exportMeetingAudio();

    expect(mockSaveMeetingAudioExport).not.toHaveBeenCalled();
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

  it("under Auto, Apple Intelligence wins over a stored Ollama model", async () => {
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
    mockApp.settings.summary_provider = "auto";
    mockApp.settings.ollama_model = "llama3";

    const ctrl = createMeetingController();
    await ctrl.mount();

    expect(ctrl.selectedModel).toBe("apple-intelligence");
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
      mockPeekSleepPausedMeeting.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockResumeMeetingRecording.mockResolvedValue(undefined);

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockResumeMeetingRecording).toHaveBeenCalledOnce());

      expect(mockPeekSleepPausedMeeting).toHaveBeenCalledOnce();
      expect(mockGetMeeting).toHaveBeenCalledWith("meet-1");
      expect(mockResumeMeetingRecording.mock.calls[0][0]).toBe("meet-1");
      expect(ctrl.statusMessage).toMatch(/resumed after sleep/i);
    });

    it("does nothing when no meeting was paused by sleep", async () => {
      mockPeekSleepPausedMeeting.mockResolvedValue(null);

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockPeekSleepPausedMeeting).toHaveBeenCalledOnce());

      expect(mockGetMeeting).not.toHaveBeenCalled();
      expect(mockResumeMeetingRecording).not.toHaveBeenCalled();
      expect(ctrl.meeting).toBeNull();
    });

    it("leaves the meeting loaded (not resumed) and surfaces the error when resume fails", async () => {
      mockPeekSleepPausedMeeting.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockResumeMeetingRecording.mockRejectedValue(new Error("model unload failed"));

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockResumeMeetingRecording).toHaveBeenCalledOnce());

      expect(ctrl.meeting?.id).toBe("meet-1");
      expect(ctrl.statusMessage).toMatch(/model unload failed/i);
    });

    // The sleep-triggered stop is spawned off the AppKit will-sleep callback
    // and routinely hasn't finished draining (EndOfStream + engine flush +
    // DB save) by the time wake fires: the machine can still read `stopping`
    // (or even `recording_meeting`) right when SystemWokeUp arrives. This is
    // the regression this ticket (SOU-040) fixes: the old destructive
    // take_sleep_paused_meeting burned the id on this very check, so the
    // resume was lost for good even though the meeting was about to become
    // resumable moments later.
    function setStoppingMeeting(meetingId = "meet-1") {
      mockApp.machineState = {
        state: "stopping",
        data: {
          profile: makeMeeting().transcription_profile,
          was_recording: { meeting: { meeting_id: meetingId } },
        },
      } as import("../../types").AppStateMachine;
    }

    function setReady() {
      mockApp.machineState = {
        state: "ready",
        data: { profile: makeMeeting().transcription_profile },
      } as import("../../types").AppStateMachine;
    }

    it("waits for the sleep-triggered stop to finish draining, then resumes once the machine reports ready", async () => {
      mockPeekSleepPausedMeeting.mockResolvedValue("meet-1");
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockResumeMeetingRecording.mockResolvedValue(undefined);
      setStoppingMeeting();

      const ctrl = createMeetingController();
      await ctrl.mount();

      notifySystemWokeUp();
      await vi.waitFor(() => expect(mockPeekSleepPausedMeeting).toHaveBeenCalledOnce());

      // Still draining: must not resume (or even load the meeting) yet.
      expect(mockResumeMeetingRecording).not.toHaveBeenCalled();
      expect(mockGetMeeting).not.toHaveBeenCalled();

      // The background stop finishes: the machine reports ready.
      setReady();
      notifyStateChanged(mockApp.machineState);

      await vi.waitFor(() => expect(mockResumeMeetingRecording).toHaveBeenCalledOnce());
      expect(mockGetMeeting).toHaveBeenCalledWith("meet-1");
      expect(mockResumeMeetingRecording.mock.calls[0][0]).toBe("meet-1");
      expect(ctrl.statusMessage).toMatch(/resumed after sleep/i);
    });

    it("does not resume or clear the flag while stopping never reaches ready, and falls back to the manual banner once the wait times out", async () => {
      vi.useFakeTimers();
      try {
        mockPeekSleepPausedMeeting.mockResolvedValue("meet-1");
        mockGetMeeting.mockResolvedValue(makeMeeting());
        setStoppingMeeting();

        const ctrl = createMeetingController();
        await ctrl.mount();

        notifySystemWokeUp();
        // Flush the pending peekSleepPausedMeeting() microtask without
        // advancing mocked time, so the wait is armed but the timeout
        // hasn't started ticking down yet.
        await Promise.resolve();
        await Promise.resolve();
        expect(mockPeekSleepPausedMeeting).toHaveBeenCalledOnce();

        // An unrelated transition that still isn't `ready` (the drain is
        // still in flight) must neither resume nor discard the id.
        setStoppingMeeting();
        notifyStateChanged(mockApp.machineState);
        expect(mockResumeMeetingRecording).not.toHaveBeenCalled();
        expect(mockClearSleepPausedMeeting).not.toHaveBeenCalled();
        expect(ctrl.meeting).toBeNull();

        // The wait cap elapses with the machine still never having reported
        // ready: give up on the automatic resume, but the id was never
        // attempted, so it must not be cleared either (only a successful
        // resume or an explicit user refusal may do that).
        await vi.advanceTimersByTimeAsync(10_000);

        expect(mockResumeMeetingRecording).not.toHaveBeenCalled();
        expect(mockClearSleepPausedMeeting).not.toHaveBeenCalled();
        // Falls back to the manual "Resume recording" banner instead of a
        // silent abandon: the meeting is loaded so canResumeRecording can
        // turn true.
        expect(ctrl.meeting?.id).toBe("meet-1");
        expect(ctrl.statusMessage).toMatch(/resume/i);
      } finally {
        vi.useRealTimers();
      }
    });

    it("shares an in-flight promise when notifySystemWokeUp is called concurrently", async () => {
      let resolvePeek: (value: string | null) => void;
      mockPeekSleepPausedMeeting.mockReturnValue(
        new Promise((resolve) => {
          resolvePeek = resolve;
        })
      );
      mockGetMeeting.mockResolvedValue(makeMeeting());
      mockResumeMeetingRecording.mockResolvedValue(undefined);

      const ctrl = createMeetingController();
      await ctrl.mount();

      // Fire two notifications concurrently (e.g. SystemWokeUp and visibilitychange)
      notifySystemWokeUp();
      notifySystemWokeUp();

      // Both calls should share the single in-flight peek
      expect(mockPeekSleepPausedMeeting).toHaveBeenCalledOnce();

      resolvePeek!("meet-1");
      await vi.waitFor(() => expect(mockResumeMeetingRecording).toHaveBeenCalledOnce());

      expect(mockGetMeeting).toHaveBeenCalledOnce();
      expect(mockResumeMeetingRecording).toHaveBeenCalledOnce();
    });
  });
});
