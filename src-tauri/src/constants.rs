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

/// Application bundle identifier
pub const APP_IDENTIFIER: &str = "com.souffle.app";

/// Get the application data directory (e.g. ~/Library/Application Support/com.souffle.app)
pub fn app_data_dir() -> std::path::PathBuf {
    dirs_next::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(APP_IDENTIFIER)
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
