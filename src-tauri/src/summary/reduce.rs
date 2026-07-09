use crate::transcript::MeetingParticipant;

use super::chunking::estimate_tokens;
use super::prompts::build_reduce_prompt;

/// Whether a reduce prompt for the given parts fits the provider token budget.
pub fn reduce_prompt_fits(
    part_summaries: &[String],
    notes: Option<&str>,
    participants: &[MeetingParticipant],
    token_limit: usize,
) -> bool {
    estimate_tokens(&build_reduce_prompt(part_summaries, notes, participants)) <= token_limit
}

/// Split map-stage summaries into batches that each fit one reduce call.
pub fn partition_for_reduce(
    part_summaries: &[String],
    token_limit: usize,
) -> Result<Vec<Vec<String>>, String> {
    if part_summaries.is_empty() {
        return Err("No part summaries to reduce.".into());
    }

    let mut batches = Vec::new();
    let mut start = 0;

    while start < part_summaries.len() {
        if !reduce_prompt_fits(&part_summaries[start..start + 1], None, &[], token_limit) {
            return Err(format!(
                "A single part summary exceeds the Apple Intelligence reduce budget \
                 (~{token_limit} tokens). Try a shorter meeting or use Ollama."
            ));
        }

        let mut end = start + 1;
        while end < part_summaries.len()
            && reduce_prompt_fits(&part_summaries[start..=end], None, &[], token_limit)
        {
            end += 1;
        }

        batches.push(part_summaries[start..end].to_vec());
        start = end;
    }

    Ok(batches)
}

/// Plan hierarchical reduce levels for tests: each batch must fit the budget.
#[cfg(test)]
pub fn plan_reduce_batches(
    part_summaries: &[String],
    config: super::chunking::ChunkConfig,
) -> Result<Vec<Vec<Vec<String>>>, String> {
    let mut levels = Vec::new();
    let mut current: Vec<String> = part_summaries.to_vec();

    loop {
        if current.len() <= 1
            && reduce_prompt_fits(&current, None, &[], config.reduce_token_limit)
        {
            break;
        }

        if reduce_prompt_fits(&current, None, &[], config.reduce_token_limit) {
            break;
        }

        let batches = partition_for_reduce(&current, config.reduce_token_limit)?;
        if batches.len() == 1 && batches[0].len() == current.len() {
            let tokens = estimate_tokens(&build_reduce_prompt(&current, None, &[]));
            return Err(format!(
                "Reduce input still exceeds Apple Intelligence budget after batching \
                 ({tokens} tokens, limit {}).",
                config.reduce_token_limit
            ));
        }

        levels.push(batches.clone());
        current = (0..batches.len())
            .map(|i| format!("Merged summary for batch {}", i + 1))
            .collect();
    }

    Ok(levels)
}

#[cfg(test)]
mod tests {
    use super::{partition_for_reduce, plan_reduce_batches, reduce_prompt_fits};
    use crate::summary::chunking::ChunkConfig;
    use crate::summary::prompts::build_reduce_prompt;

    fn simulated_map_summary(part_index: usize) -> String {
        format!(
            "Part {part_index}: {}",
            "decision action item discussion ".repeat(80)
        )
    }

    #[test]
    fn partition_batches_each_fit_budget() {
        let config = ChunkConfig::APPLE_INTELLIGENCE;
        let parts: Vec<String> = (1..=40).map(simulated_map_summary).collect();
        let batches = partition_for_reduce(&parts, config.reduce_token_limit).expect("partition");
        assert!(batches.len() > 1, "expected multiple batches for long meetings");
        for batch in &batches {
            assert!(reduce_prompt_fits(batch, None, &[], config.reduce_token_limit));
        }
    }

    #[test]
    fn hierarchical_plan_stays_under_budget_for_many_parts() {
        let config = ChunkConfig::APPLE_INTELLIGENCE;
        let parts: Vec<String> = (1..=60).map(simulated_map_summary).collect();
        let levels = plan_reduce_batches(&parts, config).expect("plan");
        assert!(
            !levels.is_empty(),
            "expected at least one batched reduce level"
        );
        for level in &levels {
            for batch in level {
                let tokens = super::estimate_tokens(&build_reduce_prompt(batch, None, &[]));
                assert!(
                    tokens <= config.reduce_token_limit,
                    "batch has {tokens} tokens, limit {}",
                    config.reduce_token_limit
                );
            }
        }
    }

    #[test]
    fn single_oversized_part_errors_early() {
        let config = ChunkConfig::APPLE_INTELLIGENCE;
        let huge = "word ".repeat(config.reduce_token_limit * 2);
        let err = partition_for_reduce(&[huge], config.reduce_token_limit).unwrap_err();
        assert!(err.contains("single part summary exceeds"));
    }
}
