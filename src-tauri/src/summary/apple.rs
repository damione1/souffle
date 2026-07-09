use crate::apple_intelligence;
use crate::constants::{
    OLLAMA_DICTATION_POLISH_PROMPT, OLLAMA_MAP_PROMPT, OLLAMA_STRUCTURED_EXTRACT_PROMPT,
    OLLAMA_SUMMARIZE_PROMPT,
};

pub const MAP_SYSTEM_PROMPT: &str = OLLAMA_MAP_PROMPT;
pub const SUMMARIZE_SYSTEM_PROMPT: &str = OLLAMA_SUMMARIZE_PROMPT;
pub const STRUCTURED_EXTRACT_SYSTEM_PROMPT: &str = OLLAMA_STRUCTURED_EXTRACT_PROMPT;
pub const DICTATION_POLISH_SYSTEM_PROMPT: &str = OLLAMA_DICTATION_POLISH_PROMPT;

pub fn validate_availability() -> Result<(), String> {
    if apple_intelligence::check_apple_intelligence_availability() {
        Ok(())
    } else {
        Err("Apple Intelligence is not available on this device.".into())
    }
}
