export type StatusReason =
  | { type: "transient"; message: string; actionLabel?: string; onAction?: () => void }
  | { type: "accessibility_denied"; message: string; actionLabel?: string; onAction?: () => void }
  | { type: "mic_denied"; message: string; actionLabel?: string; onAction?: () => void }
  | { type: "no_summary_provider"; message: string; actionLabel?: string; onAction?: () => void }
  | { type: "no_model"; message: string; actionLabel?: string; onAction?: () => void }
  | { type: "no_input_device"; message: string; actionLabel?: string; onAction?: () => void }
  | { type: "db_locked"; message: string; actionLabel?: string; onAction?: () => void };
