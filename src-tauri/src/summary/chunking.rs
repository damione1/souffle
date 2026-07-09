/// Rough token estimate without a tokenizer. Transcripts tokenize denser than prose.
pub fn estimate_tokens(text: &str) -> usize {
    (text.split_whitespace().count() as f32 * 1.4).ceil() as usize
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkConfig {
    pub stuff_token_limit: usize,
    pub chunk_words: usize,
    pub chunk_overlap_words: usize,
    pub map_concurrency: usize,
}

impl ChunkConfig {
    pub const OLLAMA: Self = Self {
        stuff_token_limit: 6000,
        chunk_words: 1400,
        chunk_overlap_words: 120,
        map_concurrency: 2,
    };

    /// Foundation Models ship with a smaller context window than local Ollama.
    pub const APPLE_INTELLIGENCE: Self = Self {
        stuff_token_limit: 1500,
        chunk_words: 450,
        chunk_overlap_words: 40,
        map_concurrency: 1,
    };
}

/// Split a transcript into overlapping word chunks for the map stage.
pub fn chunk_transcript(text: &str, config: ChunkConfig) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() <= config.chunk_words {
        return vec![text.to_string()];
    }
    let step = config
        .chunk_words
        .saturating_sub(config.chunk_overlap_words)
        .max(1);
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < words.len() {
        let end = (start + config.chunk_words).min(words.len());
        chunks.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        start += step;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::{ChunkConfig, chunk_transcript, estimate_tokens};

    #[test]
    fn estimate_tokens_scales_with_words() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens(&"word ".repeat(10)), 14);
    }

    #[test]
    fn short_transcript_is_one_chunk() {
        let text = "word ".repeat(ChunkConfig::OLLAMA.chunk_words);
        assert_eq!(chunk_transcript(&text, ChunkConfig::OLLAMA).len(), 1);
    }

    #[test]
    fn long_transcript_chunks_with_overlap() {
        let total = ChunkConfig::OLLAMA.chunk_words * 3;
        let words: Vec<String> = (0..total).map(|i| i.to_string()).collect();
        let chunks = chunk_transcript(&words.join(" "), ChunkConfig::OLLAMA);
        assert!(
            chunks.len() >= 3,
            "expected multiple chunks, got {}",
            chunks.len()
        );
        let step = ChunkConfig::OLLAMA.chunk_words - ChunkConfig::OLLAMA.chunk_overlap_words;
        assert!(chunks[1].split_whitespace().next().unwrap() == step.to_string());
        assert!(chunks[0].split_whitespace().any(|w| w == step.to_string()));
    }

    #[test]
    fn apple_chunks_are_smaller_than_ollama() {
        let total = ChunkConfig::OLLAMA.chunk_words * 3;
        let words: Vec<String> = (0..total).map(|i| i.to_string()).collect();
        let text = words.join(" ");
        let ollama = chunk_transcript(&text, ChunkConfig::OLLAMA);
        let apple = chunk_transcript(&text, ChunkConfig::APPLE_INTELLIGENCE);
        assert!(apple.len() > ollama.len());
    }
}
