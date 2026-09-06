use std::future::Future;

/// qwen2.5:7b returns literally "-" (eval_count 2) on trailing meeting
/// chunks. Anything this short, or with no letters at all, is not a fact list.
const VACUOUS_EVAL_COUNT: u32 = 5;

/// Inserted when every attempt, including a split, still produced nothing.
/// Reduce then sees an explicit hole instead of a silently dropped excerpt.
pub const EMPTY_MAP_PLACEHOLDER: &str = "- [no extractable facts from this excerpt]";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapAttempt {
    First,
    Varied,
}

impl MapAttempt {
    pub fn temperature(self) -> f32 {
        match self {
            Self::First => 0.2,
            Self::Varied => 0.7,
        }
    }

    pub fn next(self) -> Option<Self> {
        match self {
            Self::First => Some(Self::Varied),
            Self::Varied => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapOutput {
    pub text: String,
    pub eval_count: Option<u32>,
}

pub fn is_vacuous_map_output(text: &str, eval_count: Option<u32>) -> bool {
    eval_count.is_some_and(|n| n <= VACUOUS_EVAL_COUNT) || !text.chars().any(char::is_alphabetic)
}

pub fn map_user_prompt(chunk: &str, part: usize, total: usize, attempt: MapAttempt) -> String {
    match attempt {
        MapAttempt::First => {
            format!("Part {part} of {total}.\n\nTranscript excerpt:\n---\n{chunk}\n---")
        }
        MapAttempt::Varied => format!(
            "Transcript excerpt:\n---\n{chunk}\n---\n\n\
             This excerpt is one slice of a longer meeting. Extract every \
             stated fact, including logistics, scheduling, and closing remarks. \
             Never return an empty list."
        ),
    }
}

pub fn split_chunk_in_half(chunk: &str) -> Option<(String, String)> {
    let words: Vec<&str> = chunk.split_whitespace().collect();
    if words.len() < 2 {
        return None;
    }
    let mid = words.len() / 2;
    Some((words[..mid].join(" "), words[mid..].join(" ")))
}

/// Map one transcript excerpt, retrying a vacuous reply with a different
/// temperature and framing, then splitting the excerpt in two as last resort.
pub async fn map_one_chunk<G, Fut>(
    part: usize,
    total: usize,
    chunk: &str,
    generate: G,
) -> Result<String, String>
where
    G: Fn(String, f32) -> Fut,
    Fut: Future<Output = Result<MapOutput, String>>,
{
    if let Some(text) = map_until_varied(part, total, chunk, &generate).await? {
        return Ok(text);
    }

    let Some((left, right)) = split_chunk_in_half(chunk) else {
        tracing::error!(
            part,
            total,
            "Map stage produced no facts and the excerpt is too short to split; \
             inserting a placeholder so this excerpt is not dropped"
        );
        return Ok(EMPTY_MAP_PLACEHOLDER.into());
    };

    tracing::warn!(
        part,
        total,
        left_words = left.split_whitespace().count(),
        right_words = right.split_whitespace().count(),
        "Map stage still empty after retry; splitting the excerpt in two"
    );

    let left_out = map_until_varied(part, total, &left, &generate).await?;
    let right_out = map_until_varied(part, total, &right, &generate).await?;
    match (left_out, right_out) {
        (Some(left), Some(right)) => Ok(format!("{left}\n{right}")),
        (Some(text), None) | (None, Some(text)) => Ok(text),
        (None, None) => {
            tracing::error!(
                part,
                total,
                "Map stage produced no facts after retry and split; \
                 inserting a placeholder so this excerpt is not dropped"
            );
            Ok(EMPTY_MAP_PLACEHOLDER.into())
        }
    }
}

async fn map_until_varied<G, Fut>(
    part: usize,
    total: usize,
    chunk: &str,
    generate: &G,
) -> Result<Option<String>, String>
where
    G: Fn(String, f32) -> Fut,
    Fut: Future<Output = Result<MapOutput, String>>,
{
    let mut attempt = MapAttempt::First;
    loop {
        let output = generate(
            map_user_prompt(chunk, part, total, attempt),
            attempt.temperature(),
        )
        .await?;
        if !is_vacuous_map_output(&output.text, output.eval_count) {
            return Ok(Some(output.text));
        }
        tracing::warn!(
            part,
            total,
            eval_count = output.eval_count,
            ?attempt,
            "Map stage returned an empty or near-empty excerpt"
        );
        match attempt.next() {
            Some(next) => attempt = next,
            None => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EMPTY_MAP_PLACEHOLDER, MapAttempt, MapOutput, is_vacuous_map_output, map_one_chunk,
        map_user_prompt, split_chunk_in_half,
    };
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn dash_with_tiny_eval_count_is_vacuous() {
        assert!(is_vacuous_map_output("-", Some(2)));
        assert!(is_vacuous_map_output("-\n", Some(5)));
    }

    #[test]
    fn text_without_letters_is_vacuous_even_without_eval_count() {
        assert!(is_vacuous_map_output("", None));
        assert!(is_vacuous_map_output("   \n- \n", None));
        assert!(is_vacuous_map_output("42", None));
        assert!(is_vacuous_map_output("—", Some(50)));
    }

    #[test]
    fn short_eval_count_is_vacuous_even_with_letters() {
        assert!(is_vacuous_map_output("OK", Some(2)));
        assert!(is_vacuous_map_output("OK", Some(5)));
    }

    #[test]
    fn a_real_fact_list_is_not_vacuous() {
        assert!(!is_vacuous_map_output("- HopSpot ships in July", Some(40)));
        assert!(!is_vacuous_map_output("- milestone en août", Some(6)));
        assert!(!is_vacuous_map_output("é", Some(10)));
    }

    #[test]
    fn varied_attempt_changes_temperature_and_framing() {
        let chunk = "closing remarks about July";
        let first = map_user_prompt(chunk, 5, 5, MapAttempt::First);
        let varied = map_user_prompt(chunk, 5, 5, MapAttempt::Varied);
        assert_eq!(MapAttempt::First.temperature(), 0.2);
        assert_eq!(MapAttempt::Varied.temperature(), 0.7);
        assert!(first.contains("Part 5 of 5"));
        assert!(!varied.contains("Part 5 of 5"));
        assert!(varied.contains("Never return an empty list"));
        assert_ne!(first, varied);
    }

    #[test]
    fn split_halves_keep_every_word() {
        let (left, right) = split_chunk_in_half("one two three four").unwrap();
        assert_eq!(left, "one two");
        assert_eq!(right, "three four");
        assert!(split_chunk_in_half("solo").is_none());
    }

    fn vacuous() -> MapOutput {
        MapOutput {
            text: "-".into(),
            eval_count: Some(2),
        }
    }

    fn fact(text: &str) -> MapOutput {
        MapOutput {
            text: text.into(),
            eval_count: Some(40),
        }
    }

    #[tokio::test]
    async fn a_good_first_output_is_not_retried() {
        let calls = AtomicU32::new(0);
        let result = map_one_chunk(1, 5, "meeting talk", |_prompt, temperature| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move {
                assert!((temperature - 0.2).abs() < f32::EPSILON);
                Ok(fact("- decided to ship"))
            }
        })
        .await
        .unwrap();
        assert_eq!(result, "- decided to ship");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn vacuous_first_attempt_retries_with_variation() {
        let calls = AtomicU32::new(0);
        let result = map_one_chunk(5, 5, "closing remarks about July", |prompt, temperature| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    assert!((temperature - 0.2).abs() < f32::EPSILON);
                    assert!(prompt.contains("Part 5 of 5"));
                    Ok(vacuous())
                } else {
                    assert!((temperature - 0.7).abs() < f32::EPSILON);
                    assert!(!prompt.contains("Part 5 of 5"));
                    Ok(fact("- HopSpot ships in July"))
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result, "- HopSpot ships in July");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn two_vacuous_attempts_split_the_chunk() {
        let calls = AtomicU32::new(0);
        let result = map_one_chunk(5, 5, "alpha bravo charlie delta", |prompt, _temperature| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                // Whole chunk: First + Varied both empty, then each half succeeds.
                if n < 2 {
                    assert!(prompt.contains("alpha bravo charlie delta"));
                    Ok(vacuous())
                } else if prompt.contains("alpha bravo") && !prompt.contains("charlie") {
                    Ok(fact("- first half"))
                } else {
                    Ok(fact("- second half"))
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(result, "- first half\n- second half");
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn a_successful_split_half_keeps_that_half() {
        let result = map_one_chunk(
            5,
            5,
            "alpha bravo charlie delta",
            |prompt, _temperature| async move {
                if prompt.contains("charlie delta") && !prompt.contains("alpha") {
                    Ok(fact("- second half"))
                } else {
                    Ok(vacuous())
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "- second half");
    }

    #[tokio::test]
    async fn split_failure_still_returns_a_placeholder() {
        let result = map_one_chunk(
            5,
            5,
            "alpha bravo charlie delta",
            |_prompt, _temperature| async move { Ok(vacuous()) },
        )
        .await
        .unwrap();
        assert_eq!(result, EMPTY_MAP_PLACEHOLDER);
    }

    #[tokio::test]
    async fn generate_errors_are_not_swallowed_as_empty() {
        let err = map_one_chunk(1, 1, "talk", |_prompt, _temperature| async move {
            Err("Ollama error: 500".into())
        })
        .await
        .unwrap_err();
        assert!(err.contains("500"));
    }
}
