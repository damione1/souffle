export type TranscriptionSegment = {
  text: string;
  start_time: number;
  end_time: number;
  is_final: boolean;
  language?: string;
  confidence?: number;
};

export type ModelStatus = {
  profile: TranscriptionProfile;
  downloaded: boolean;
  loaded: boolean;
  model_dir: string;
};

export type TranscriptionProfile = {
  engine_id: string;
  engine_label: string;
  model_id: string;
  model_label: string;
};

export type TranscriptionModelDescriptor = {
  id: string;
  label: string;
  description: string;
  download_size_bytes: number | null;
  supported_languages: string[];
};

export type TranscriptionEngineDescriptor = {
  id: string;
  label: string;
  description: string;
  supports_streaming: boolean;
  models: TranscriptionModelDescriptor[];
};

export type TranscriptionCatalog = {
  engines: TranscriptionEngineDescriptor[];
  selected_engine_id: string;
  selected_model_id: string;
};

export type DownloadProgress = {
  file: string;
  downloaded_bytes: number;
  total_bytes: number | null;
  status: "downloading" | "complete" | { error: string };
};

export type AudioDevice = {
  name: string;
  is_default: boolean;
};

export type MeetingTranscript = {
  id: string;
  title: string;
  started_at: string;
  ended_at: string | null;
  duration_seconds: number;
  transcription_profile: TranscriptionProfile;
  engine: string;
  segments: TranscriptionSegment[];
  summary: string | null;
  summary_model: string | null;
  summary_generated_at: string | null;
};

export type MeetingListItem = {
  id: string;
  title: string;
  started_at: string;
  duration_seconds: number;
  has_summary: boolean;
};

export type OllamaModelDescriptor = {
  id: string;
  label: string;
  can_summarize: boolean;
};

export type OllamaStatus = {
  available: boolean;
  base_url: string;
  models: OllamaModelDescriptor[];
};

export type SummarizeProgress = {
  text: string;
  done: boolean;
};

export type Theme = "dark" | "light" | "system";

export type AppSettings = {
  theme: Theme;
  auto_paste: boolean;
  paste_delay_ms: number;
  ollama_url: string;
  ollama_model: string;
  debug_transcription: boolean;
  transcription_engine_id: string;
  transcription_model_id: string;
};

export type PersistedAppSettings = AppSettings & {
  audio_device: string | null;
};

export type ShortcutSettings = {
  toggle: string;
  push_to_talk: string;
};

export type DictationEntry = {
  id: string;
  text: string;
  timestamp: string; // ISO date
};

export type View = "transcription" | "meeting" | "meeting-history" | "settings";
