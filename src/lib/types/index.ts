export type TranscriptionSegment = {
  text: string;
  start_time: number;
  end_time: number;
  is_final: boolean;
  language?: string;
  confidence?: number;
};

export type ModelStatus = {
  downloaded: boolean;
  loaded: boolean;
  model_dir: string;
  engine_name: string;
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

export type OllamaStatus = {
  available: boolean;
  models: string[];
  summary_models: string[];
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
};

export type DictationEntry = {
  id: string;
  text: string;
  timestamp: string; // ISO date
};

export type View = "transcription" | "meeting" | "meeting-history" | "settings";
