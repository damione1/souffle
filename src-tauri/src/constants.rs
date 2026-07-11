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

/// System prompt for the FINAL summarization pass via Ollama (either the whole
/// transcript in one shot, or the fully merged map-reduce notes). This is the
/// only pass allowed to emit the user-facing markdown structure: it must run
/// exactly once per summary, never per chunk or per intermediate reduce round,
/// or its headings repeat once concatenated with sibling groups.
/// Decisions/action items/open questions are extracted separately by
/// OLLAMA_STRUCTURED_EXTRACT_PROMPT and rendered in their own UI section, so
/// this prose pass stays a narrative recap and does not duplicate them.
pub const OLLAMA_SUMMARIZE_PROMPT: &str = "\
You summarize a meeting transcript (or notes already extracted from one) into a short, factual narrative recap.

Rules:
- Use only information explicitly stated in the input.
- Never infer project names, decisions, action items, deadlines, owners, next steps, or meeting outcomes beyond what is stated.
- Do not turn greetings or a short introduction into a broader meeting narrative.
- Do not greet, thank, apologize, or address the reader.
- Do not add an introduction, conclusion, or commentary about the meeting quality.
- Give equal weight to the beginning, middle, and end of the meeting; do not over-emphasize the final portion.
- If the input is very short, output only the facts that are directly present.
- If the input only contains greetings, attendance, or setup, say only that.
- Keep the markdown section headings exactly as written below in English.
- Write the content of each bullet in the same language as the input.
- If a section has no content, write a short \"none stated\" equivalent in the same language.
- Use short, concrete bullets. No paragraphs.
- Decisions, action items, and open questions are extracted separately elsewhere: do not add sections for them here.
- Return exactly this structure and nothing before or after it:

## Summary
- ...

## Topics
- ...";

/// System prompt for the per-chunk "map" stage when a transcript is too long to
/// summarize in one pass. Each chunk is extracted independently into a flat,
/// unheaded bullet list; a single flat list (not headed sections) is the point:
/// merging headed sections from several chunks concatenates the headings and
/// they repeat in the final text. The chunk notes are later combined by
/// OLLAMA_MERGE_PROMPT (one or more rounds), and only the truly final pass
/// (OLLAMA_SUMMARIZE_PROMPT) renders the user-facing structure, exactly once.
pub const OLLAMA_MAP_PROMPT: &str = "\
You are extracting facts from ONE part of a longer meeting transcript.
Capture only what is in this excerpt. Do not speculate about other parts.

Rules:
- Use only information explicitly stated in this excerpt.
- Do not greet, thank, or address the reader. No preamble.
- Write dense, factual bullet points in the same language as the transcript.
- One bullet per distinct fact: a topic discussed, a decision made, an action item assigned (name the owner if stated), or an open question or risk raised.
- Do NOT use section headings or labels such as \"Decisions:\" or \"Topics:\". Output a single flat bullet list.
- No paragraphs, no commentary, no summary sentence.";

/// System prompt for intermediate reduce rounds, used only when a meeting has
/// too many map-stage chunks to combine in a single reduce call (mainly Apple
/// Intelligence's small context window). Output stays a flat, unheaded bullet
/// list like the map stage: it must NOT render the final markdown structure,
/// otherwise every intermediate round adds another copy of the headings by the
/// time the last round runs.
pub const OLLAMA_MERGE_PROMPT: &str = "\
You are merging several already-extracted fact lists from consecutive parts of ONE meeting transcript into a single combined list.

Rules:
- Use only information explicitly stated in the parts below.
- Do not greet, thank, or address the reader. No preamble.
- Write dense, factual bullet points in the same language as the parts.
- Merge duplicate or overlapping points into a single bullet instead of repeating them.
- Keep the parts in their given order; do not over-weight the last part.
- Do NOT use section headings or labels such as \"Decisions:\" or \"Topics:\". Output a single flat bullet list.
- No paragraphs, no commentary, no summary sentence.";

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

/// Built-in final-pass prompt for the "Detailed minutes" summary template
/// (see `crate::summary::default_summary_templates`). Same rules as
/// [`OLLAMA_SUMMARIZE_PROMPT`], but asks for a thorough, chronological
/// account instead of a short recap.
pub const OLLAMA_DETAILED_MINUTES_PROMPT: &str = "\
You produce detailed, structured meeting minutes from a meeting transcript (or notes already extracted from one).

Rules:
- Use only information explicitly stated in the input.
- Never infer project names, decisions, action items, deadlines, owners, next steps, or meeting outcomes beyond what is stated.
- Do not turn greetings or a short introduction into a broader meeting narrative.
- Do not greet, thank, apologize, or address the reader.
- Do not add an introduction, conclusion, or commentary about the meeting quality.
- Give equal weight to the beginning, middle, and end of the meeting; do not over-emphasize the final portion.
- Cover the meeting in chronological order with one bullet per distinct point discussed; be thorough rather than terse.
- If the input is very short, output only the facts that are directly present.
- If the input only contains greetings, attendance, or setup, say only that.
- Keep the markdown section headings exactly as written below in English.
- Write the content of each bullet in the same language as the input.
- If a section has no content, write a short \"none stated\" equivalent in the same language.
- Decisions, action items, and open questions are extracted separately elsewhere: do not add sections for them here.
- Return exactly this structure and nothing before or after it:

## Meeting Minutes
- ...

## Topics
- ...";

/// Built-in final-pass prompt for the "Brief overview" summary template
/// (see `crate::summary::default_summary_templates`). Same rules as
/// [`OLLAMA_SUMMARIZE_PROMPT`], but caps the output at a handful of bullets.
pub const OLLAMA_BRIEF_OVERVIEW_PROMPT: &str = "\
You summarize a meeting transcript (or notes already extracted from one) into the shortest possible factual overview.

Rules:
- Use only information explicitly stated in the input.
- Never infer project names, decisions, action items, deadlines, owners, next steps, or meeting outcomes beyond what is stated.
- Do not turn greetings or a short introduction into a broader meeting narrative.
- Do not greet, thank, apologize, or address the reader.
- Do not add an introduction, conclusion, or commentary about the meeting quality.
- Give equal weight to the beginning, middle, and end of the meeting; do not over-emphasize the final portion.
- Write at most 3 short bullets covering only the most important points.
- If the input only contains greetings, attendance, or setup, say only that.
- Keep the markdown section heading exactly as written below in English.
- Write the content of each bullet in the same language as the input.
- If the input has no substantive content, write a short \"none stated\" equivalent in the same language.
- Decisions, action items, and open questions are extracted separately elsewhere: do not add a section for them here.
- Return exactly this structure and nothing before or after it:

## Summary
- ...";
