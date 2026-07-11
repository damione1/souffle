import type {
  AppSettings,
  DictationEntry,
  MeetingListItem,
  MeetingTranscript,
  SummaryProvidersStatus,
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
      models: [
        {
          id: "stt-1b-en_fr",
          label: "STT 1B EN/FR",
          description: "1-billion parameter English/French model",
          download_size_bytes: 2_400_000_000,
          recommended_memory_bytes: 4_000_000_000,
          supported_languages: ["en", "fr"],
          capabilities: {
            supports_streaming: true,
            supports_batch_transcription: false,
            supports_language_auto_detect: true,
            supports_word_timestamps: true,
            supports_partial_results: true,
          },
          audio_input: {
            sample_rate_hz: 24_000,
            channels: 1,
            chunk_size_samples: 1_920,
          },
          available_in_app: true,
          availability_note: null,
          backends: [
            {
              id: "candle",
              label: "Candle",
              description: "Pure Rust runtime used by Souffle for local transcription.",
              recommended: true,
              available_in_app: true,
              availability_note: null,
              artifacts: [
                {
                  id: "hf-candle-stt-1b-en-fr",
                  label: "Hugging Face",
                  description: "Hugging Face Candle export for the Kyutai 1B FR/EN model.",
                  provider: "huggingface",
                  repository: "kyutai/stt-1b-en_fr-candle",
                  revision: null,
                  file_format: "safetensors",
                  download_size_bytes: 2_400_000_000,
                  required_files: ["config.json", "model.safetensors"],
                },
              ],
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

export const mockRuntimeStatus: TranscriptionRuntimeStatus = {
  profile: {
    engine_id: "kyutai",
    engine_label: "Kyutai STT",
    model_id: "stt-1b-en_fr",
    model_label: "STT 1B EN/FR",
    backend_id: "candle",
    backend_label: "Candle",
  },
  phase: "ready",
  model_dir: "/mock/models/kyutai/stt-1b-en_fr/candle",
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
    backend_id: "candle",
    backend_label: "Candle",
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
  structured_summary: null,
  edited_transcript: null,
  notes: null,
  calendar_event_id: null,
  participants: [],
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
  locale: "",
  auto_paste: false,
  paste_delay_ms: 100,
  paste_method: "clipboard",
  ollama_url: "http://localhost:11434",
  ollama_model: "",
  debug_transcription: false,
  log_level: "info",
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
  calendar_autostart_enabled: true,
  feedback_sounds_enabled: true,
  feedback_sounds_volume: 70,
  model_unload_timeout_minutes: 0,
  meeting_autostop_enabled: true,
  meeting_autostop_minutes: 10,
  meeting_max_duration_minutes: 240,
  meeting_audio_retention: "off",
  dictation_polish_enabled: false,
  dictation_polish_template_id: "email",
  dictation_polish_templates: [
    { id: "email", label: "Professional email", prompt: "Rewrite as email." },
    { id: "bullets", label: "Bullet points", prompt: "Use bullets." },
    { id: "no_fillers", label: "Remove fillers", prompt: "Remove fillers." },
  ],
  last_seen_version: "",
}

export const mockShortcuts: ShortcutSettings = {
  toggle: "CommandOrControl+Shift+S",
  push_to_talk: "CommandOrControl+Shift+Space",
};

// ---------------------------------------------------------------------------
// Summary providers
// ---------------------------------------------------------------------------

export const mockSummaryProvidersStatus: SummaryProvidersStatus = {
  ollama_url: "http://localhost:11434",
  ollama_available: true,
  apple_intelligence_available: false,
  apple_intelligence_is_stub: true,
  apple_intelligence_unavailable_reason: "stub",
  models: [
    { id: "llama3.2:latest", label: "Llama 3.2", provider: "ollama", can_summarize: true },
    { id: "mistral:latest", label: "Mistral", provider: "ollama", can_summarize: true },
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
