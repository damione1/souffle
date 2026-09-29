//! "Learn from my corrections" (`dictation_learn_from_edit`): port of
//! `scheduleLearnFromEdit` in the Tauri-era
//! `features/transcription/controller.svelte.ts`. A few seconds after an
//! auto-paste, the focused field of the app the text went into is read back
//! through Accessibility; if the user fixed a few words, those word pairs go
//! to `learn_from_edit`, which keeps its own guard rails (SOU-066).
//!
//! Everything here is best-effort, as it was in Tauri: a failed read or
//! write is logged and dropped, never surfaced to the user.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use crate::AppHandle;

/// Time the user gets to fix the pasted text before it is read back.
const LEARN_FROM_EDIT_DELAY: Duration = Duration::from_secs(4);

/// More changed word pairs than this is a rewrite, not recognition fixes.
const MAX_LEARN_FROM_EDIT_PAIRS: usize = 8;

/// Bumped by every schedule and every cancel: a pending read whose
/// generation is no longer current belongs to an older dictation and stops.
static GENERATION: AtomicU64 = AtomicU64::new(0);

/// Drop the pending post-paste read, if any. A new dictation starting must
/// not have the previous one's read land on top of it.
pub(crate) fn cancel_pending() {
    GENERATION.fetch_add(1, Ordering::AcqRel);
}

/// Called after a successful auto-paste of `pasted` into `target_app` (the
/// frontmost app when the dictation was stopped). Replaces any read still
/// pending from an earlier paste.
pub(crate) fn schedule(
    handle: AppHandle,
    enabled: bool,
    pasted: String,
    target_app: Option<String>,
) {
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel) + 1;
    if !enabled || pasted.is_empty() {
        return;
    }
    souffle_lib::async_runtime::spawn(async move {
        tokio::time::sleep(LEARN_FROM_EDIT_DELAY).await;
        if GENERATION.load(Ordering::Acquire) != generation {
            return;
        }
        // The user may have turned the toggle off during the delay.
        let still_enabled = souffle_lib::commands::get_settings(std::sync::Arc::clone(&handle))
            .map(|settings| settings.dictation_learn_from_edit)
            .unwrap_or(false);
        if !still_enabled {
            return;
        }
        // NSWorkspace wants the main thread, like the stop-time read.
        let current_app = crate::run_on_main_thread(|| {
            souffle_lib::commands::frontmost_app_name().unwrap_or(None)
        })
        .await;
        if !is_same_app(target_app.as_deref(), current_app.as_deref()) {
            return;
        }
        // The AX read can block for the target app's messaging timeout if
        // it hangs, so it stays off the UI thread and off the async workers
        // (the paste itself already talks to AX from a worker). No
        // Accessibility grant reads back as `None`.
        let worker = souffle_lib::async_runtime::spawn_blocking(move || {
            let focused = souffle_lib::commands::read_focused_text()?;
            let Some(corrected) = edited_text_to_learn(&pasted, focused.as_deref()) else {
                return Ok(0);
            };
            souffle_lib::commands::learn_from_edit(handle, pasted, corrected)
        });
        match worker.await {
            Ok(Ok(_)) => {}
            Ok(Err(error)) => eprintln!("Learn from edit failed: {error}"),
            Err(error) => eprintln!("Learn from edit worker failed: {error}"),
        }
    });
}

/// The read-back only means something in the app the text was pasted into.
/// An unknown target never matches.
fn is_same_app(target_app: Option<&str>, current_app: Option<&str>) -> bool {
    matches!((target_app, current_app), (Some(target), Some(current)) if target == current)
}

/// The corrected text to learn from, or `None` when the focused field shows
/// nothing to learn: empty, unchanged, or clearly more than the paste. An
/// AX read returns the whole field, so pre-existing text around the paste
/// would otherwise turn into a batch of unrelated word pairs.
fn edited_text_to_learn(pasted: &str, focused: Option<&str>) -> Option<String> {
    let focused = focused.map(str::trim).filter(|text| !text.is_empty())?;
    if focused == pasted {
        return None;
    }
    if tokenize_words(focused).len() > tokenize_words(pasted).len() + 3 {
        return None;
    }
    // Tauri compared UTF-16 lengths; chars are the same for the text a
    // dictation produces and close enough elsewhere.
    let focused_len = focused.chars().count() as f64;
    let pasted_len = pasted.chars().count() as f64;
    if focused_len > pasted_len * 1.5 + 20.0 {
        return None;
    }
    if count_correction_pairs(pasted, focused) > MAX_LEARN_FROM_EDIT_PAIRS {
        return None;
    }
    Some(focused.to_string())
}

/// Whitespace tokens with leading/trailing non-alphanumerics stripped.
fn tokenize_words(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|token| !token.is_empty())
        .collect()
}

/// Word-level changed-pair count, same alignment as
/// `derive_corrections_from_edit` but with the looser pair test the Tauri
/// caller used (both words at least 3 characters and different), so the
/// rewrite cut-off counts every changed word, not only the learnable ones.
fn count_correction_pairs(original: &str, corrected: &str) -> usize {
    let orig: Vec<String> = tokenize_words(original)
        .into_iter()
        .map(str::to_lowercase)
        .collect();
    let corr: Vec<String> = tokenize_words(corrected)
        .into_iter()
        .map(str::to_lowercase)
        .collect();
    if orig.is_empty() || corr.is_empty() {
        return 0;
    }

    let mut seen = std::collections::HashSet::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < orig.len() && j < corr.len() {
        if orig[i] == corr[j] {
            i += 1;
            j += 1;
            continue;
        }
        if j + 1 < corr.len() && orig[i] == corr[j + 1] {
            j += 1;
            continue;
        }
        if i + 1 < orig.len() && orig[i + 1] == corr[j] {
            i += 1;
            continue;
        }
        if orig[i].chars().count() >= 3 && corr[j].chars().count() >= 3 {
            seen.insert((orig[i].as_str(), corr[j].as_str()));
        }
        i += 1;
        j += 1;
    }
    seen.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_app_needs_both_names_and_a_match() {
        assert!(is_same_app(Some("Notes"), Some("Notes")));
        assert!(!is_same_app(Some("Notes"), Some("Mail")));
        assert!(!is_same_app(None, Some("Notes")));
        assert!(!is_same_app(Some("Notes"), None));
        assert!(!is_same_app(None, None));
    }

    #[test]
    fn a_small_fix_is_learned_from_the_trimmed_field() {
        assert_eq!(
            edited_text_to_learn(
                "Envoie le rapport à Gaelle demain",
                Some("  Envoie le rapport à Gaël demain \n"),
            ),
            Some("Envoie le rapport à Gaël demain".to_string()),
        );
    }

    #[test]
    fn empty_unreadable_or_unchanged_fields_are_skipped() {
        let pasted = "Envoie le rapport demain";
        assert_eq!(edited_text_to_learn(pasted, None), None);
        assert_eq!(edited_text_to_learn(pasted, Some("   ")), None);
        assert_eq!(
            edited_text_to_learn(pasted, Some(" Envoie le rapport demain ")),
            None
        );
    }

    #[test]
    fn a_field_with_more_than_three_extra_words_is_skipped() {
        let pasted = "Envoie le rapport demain";
        assert!(
            edited_text_to_learn(pasted, Some("Envoie le rapport complet demain matin")).is_some()
        );
        assert_eq!(
            edited_text_to_learn(
                pasted,
                Some("Bonjour Marc, envoie le rapport demain stp merci")
            ),
            None,
        );
    }

    #[test]
    fn a_field_much_longer_than_the_paste_is_skipped() {
        // Same word count, but the words are far longer than what was pasted.
        let pasted = "un deux trois quatre";
        assert_eq!(
            edited_text_to_learn(
                pasted,
                Some("anticonstitutionnellement extraordinairement incompréhensiblement quatre"),
            ),
            None,
        );
    }

    #[test]
    fn a_rewrite_with_more_than_eight_changed_words_is_skipped() {
        let pasted = "alpha bravo charlie delta echo foxtrot golf hotel india";
        let rewritten = "zulu yankee xray whiskey victor uniform tango sierra romeo";
        assert_eq!(count_correction_pairs(pasted, rewritten), 9);
        assert_eq!(edited_text_to_learn(pasted, Some(rewritten)), None);

        let eight = "zulu yankee xray whiskey victor uniform tango sierra india";
        assert_eq!(count_correction_pairs(pasted, eight), 8);
        assert_eq!(
            edited_text_to_learn(pasted, Some(eight)),
            Some(eight.to_string())
        );
    }

    #[test]
    fn pair_count_ignores_case_short_words_insertions_and_repeats() {
        assert_eq!(
            count_correction_pairs("Bonjour Gaelle", "bonjour GAELLE"),
            0
        );
        assert_eq!(count_correction_pairs("le chat", "la chat"), 0);
        assert_eq!(
            count_correction_pairs("envoie le rapport", "envoie vite le rapport"),
            0
        );
        assert_eq!(
            count_correction_pairs("Gaelle et Gaelle", "Gaël et Gaël"),
            1,
            "the same pair twice counts once",
        );
        assert_eq!(count_correction_pairs("", "quelque chose"), 0);
    }
}
