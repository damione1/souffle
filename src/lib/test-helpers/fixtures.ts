import type {
  AppSettings,
  DictationEntry,
  MeetingListItem,
  MeetingTranscript,
  OllamaStatus,
  ShortcutSettings,
  TranscriptionCatalog,
  TranscriptionRuntimeStatus,
  TranscriptionSegment,
} from "../types";

// ---------------------------------------------------------------------------
// Transcription
// ---------------------------------------------------------------------------

export const mockCatalog: TranscriptionCatalog = {
  engines: [
    {
      id: "kyutai",
      label: "Kyutai STT",
      description: "Kyutai speech-to-text (stt-1b-en_fr) with Metal acceleration",
      supports_streaming: true,
      models: [
        {
          id: "stt-1b-en_fr",
          label: "STT 1B EN/FR",
          description: "1-billion parameter English/French model",
          download_size_bytes: 2_400_000_000,
          supported_languages: ["en", "fr"],
        },
      ],
    },
  ],
  selected_engine_id: "kyutai",
  selected_model_id: "stt-1b-en_fr",
};

export const mockRuntimeStatus: TranscriptionRuntimeStatus = {
  profile: {
    engine_id: "kyutai",
    engine_label: "Kyutai STT",
    model_id: "stt-1b-en_fr",
    model_label: "STT 1B EN/FR",
  },
  downloaded: true,
  loaded: true,
  model_dir: "/mock/models/kyutai/stt-1b-en_fr",
};

export const mockSegment: TranscriptionSegment = {
  text: "Hello world",
  start_time: 0.0,
  end_time: 1.5,
  is_final: true,
  language: "en",
  confidence: 0.95,
};

// ---------------------------------------------------------------------------
// Meetings
// ---------------------------------------------------------------------------

export const mockMeeting: MeetingTranscript = {
  id: "meeting-001",
  title: "Test Meeting",
  started_at: "2025-06-01T10:00:00Z",
  ended_at: "2025-06-01T10:30:00Z",
  duration_seconds: 1800,
  transcription_profile: {
    engine_id: "kyutai",
    engine_label: "Kyutai STT",
    model_id: "stt-1b-en_fr",
    model_label: "STT 1B EN/FR",
  },
  recording_sessions: [
    {
      id: "session-001",
      started_at: "2025-06-01T10:00:00Z",
      ended_at: "2025-06-01T10:30:00Z",
      duration_seconds: 1800,
      start_segment_index: 0,
      end_segment_index: 2,
    },
  ],
  segments: [
    {
      text: "Welcome to the meeting.",
      start_time: 0.0,
      end_time: 2.0,
      is_final: true,
      language: "en",
      confidence: 0.92,
    },
    {
      text: "Let's discuss the roadmap.",
      start_time: 2.5,
      end_time: 4.5,
      is_final: true,
      language: "en",
      confidence: 0.88,
    },
  ],
  summary: null,
  summary_is_stale: false,
  summary_model: null,
  summary_generated_at: null,
};

export const mockMeetingList: MeetingListItem[] = [
  {
    id: "meeting-001",
    title: "Test Meeting",
    started_at: "2025-06-01T10:00:00Z",
    duration_seconds: 1800,
    has_summary: false,
    summary_is_stale: false,
  },
  {
    id: "meeting-002",
    title: "Sprint Retrospective",
    started_at: "2025-06-02T14:00:00Z",
    duration_seconds: 3600,
    has_summary: true,
    summary_is_stale: false,
  },
];

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

export const mockSettings: AppSettings = {
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

export const mockShortcuts: ShortcutSettings = {
  toggle: "CommandOrControl+Shift+S",
  push_to_talk: "CommandOrControl+Shift+Space",
};

// ---------------------------------------------------------------------------
// Ollama
// ---------------------------------------------------------------------------

export const mockOllamaStatus: OllamaStatus = {
  available: true,
  base_url: "http://localhost:11434",
  models: [
    { id: "llama3.2:latest", label: "Llama 3.2", can_summarize: true },
    { id: "mistral:latest", label: "Mistral", can_summarize: true },
  ],
};

// ---------------------------------------------------------------------------
// Dictation
// ---------------------------------------------------------------------------

export const mockDictationEntry: DictationEntry = {
  id: "dict-001",
  text: "The quick brown fox jumps over the lazy dog.",
  timestamp: "2025-06-01T09:00:00Z",
};
