/// Audio pipeline sample rate (Mimi codec expects 24kHz)
pub const SAMPLE_RATE: u32 = 24_000;

/// Sample rate as f64 for duration calculations
pub const SAMPLE_RATE_F64: f64 = 24_000.0;

/// Last-resort timeout waiting for the engine actor's stop reply (seconds).
/// The stop itself is event-ordered (EndOfStream marker); this only fires if
/// the actor is wedged. Generous because the final drain can include real
/// inference work on buffered audio.
pub const STOP_REPLY_TIMEOUT_SECS: u64 = 15;

/// 1.5 seconds of silence at 24kHz — used as suffix for flushing
pub const SILENCE_SUFFIX_SAMPLES: usize = 36_000;

/// Mimi codec frame size: 1920 samples = 80ms at 24kHz
pub const MIMI_FRAME_SIZE: usize = 1920;

/// Mimi codec frame rate (24000 / 1920)
pub const MIMI_FRAMES_PER_SECOND: f64 = 12.5;

/// Application bundle identifier
pub const APP_IDENTIFIER: &str = "com.souffle.desktop";

/// Former bundle identifier; the data directory is renamed from this to
/// [`APP_IDENTIFIER`] at startup so existing meetings/settings/models survive
/// the change. (The old `.app` suffix conflicted with the macOS bundle
/// extension.)
pub const LEGACY_APP_IDENTIFIER: &str = "com.souffle.app";

/// Get the application data directory (e.g. ~/Library/Application Support/com.souffle.desktop)
pub fn app_data_dir() -> std::path::PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(APP_IDENTIFIER)
}

/// Rename the legacy data directory to the current one if it exists and the new
/// one doesn't. Runs before anything opens the database or log files. Best
/// effort: on failure the app simply starts fresh at the new path.
pub fn migrate_legacy_data_dir() {
    let Some(base) = dirs_next::data_dir() else {
        return;
    };
    let old = base.join(LEGACY_APP_IDENTIFIER);
    let new = base.join(APP_IDENTIFIER);
    if old.exists()
        && !new.exists()
        && let Err(e) = std::fs::rename(&old, &new)
    {
        eprintln!("Failed to migrate data dir {old:?} -> {new:?}: {e}");
    }
}

/// Default Ollama server URL
pub const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

/// System prompt for meeting summarization via Ollama
pub const OLLAMA_SUMMARIZE_PROMPT: &str = "\
You summarize meeting transcripts into a factual, extractive outline.

Rules:
- Use only information explicitly stated in the transcript.
- Never infer project names, decisions, action items, deadlines, owners, next steps, or meeting outcomes.
- Do not turn greetings or a short introduction into a broader meeting narrative.
- Do not greet, thank, apologize, or address the reader.
- Do not add an introduction, conclusion, or commentary about the meeting quality.
- Give equal weight to the beginning, middle, and end of the meeting; do not over-emphasize the final portion.
- If the transcript is very short, output only the facts that are directly present.
- If the transcript only contains greetings, attendance, or setup, say only that.
- Keep the markdown section headings exactly as written below in English.
- Write the content of each bullet in the same language as the transcript.
- If a section has no content in the transcript, write a short \"none stated in transcript\" equivalent in the same language.
- Use short, concrete bullets. No paragraphs.
- Return exactly this structure and nothing before or after it:

## Summary
- ...

## Decisions
- ...

## Action Items
- ...

## Topics
- ...";

/// System prompt for the per-chunk "map" stage when a transcript is too long to
/// summarize in one pass. Each chunk is summarized independently, then the
/// chunk summaries are combined by a final pass using OLLAMA_SUMMARIZE_PROMPT.
/// Keeping this extraction-only (no fixed skeleton) lets the reduce stage own
/// the final structure.
pub const OLLAMA_MAP_PROMPT: &str = "\
You are summarizing ONE part of a longer meeting transcript.
Summarize ONLY what is in this excerpt. Do not speculate about other parts.

Rules:
- Use only information explicitly stated in this excerpt.
- Do not greet, thank, or address the reader. No preamble.
- Write bullets in the same language as the transcript.
- Be concise: short, concrete bullets.

Extract, using exactly these headings:

Topics:
- ...

Decisions:
- ... (or \"None\")

Action Items:
- ... (owner — task, if stated; or \"None\")

Open Questions / Risks:
- ... (or \"None\")";

/// System prompt for dictation post-processing (polish) via Ollama.
pub const OLLAMA_DICTATION_POLISH_PROMPT: &str = "\
You post-process speech-to-text dictation according to the user's instructions.

Rules:
- Apply only the requested transformation to the transcript.
- Preserve the original language unless explicitly asked to translate.
- Do not greet, apologize, or add commentary about the task.
- Return ONLY valid JSON with exactly one key: text (string).
- No markdown, no code fences, no text before or after the JSON object.";

/// System prompt for the structured extraction pass (decisions, action items,
/// open questions) run after the prose summary is generated.
pub const OLLAMA_STRUCTURED_EXTRACT_PROMPT: &str = "\
You extract structured meeting outcomes from a prose meeting summary.

Rules:
- Use only information explicitly stated in the summary or user notes.
- Never invent decisions, owners, deadlines, or questions.
- Return ONLY valid JSON. No markdown, no commentary, no code fences.
- Use exactly these keys: decisions, action_items, open_questions.
- decisions and open_questions are string arrays.
- action_items is an array of objects with text (string) and owner (string or null).
- Use null for unknown owners. Use empty arrays when a category has no items.";
