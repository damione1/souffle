//! Port of the key-points extraction in `MeetingSummarySection.svelte`
//! (`keyPoints` `$derived.by`) - milestone 9. Read-only display support
//! only: generating a summary isn't wired here yet, see the doc comment on
//! `components/summary_section.slint`.

/// Pulls up to 4 bullet/numbered lines out of a raw summary, stripping
/// their leading marker - same two regexes as the Svelte reference
/// (`/^[-•*]\s/` and `/^\d+[.)]\s/`), reimplemented without a regex crate
/// since the patterns are this simple.
pub fn extract_key_points(summary: &str) -> Vec<String> {
    summary
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter_map(strip_marker)
        .take(4)
        .collect()
}

/// Strips a leading `-`, `•`, `*`, or `N.`/`N)` marker (plus the space after
/// it) from `line`, returning `None` if `line` doesn't start with one of
/// those markers at all.
fn strip_marker(line: &str) -> Option<String> {
    if let Some(rest) = line
        .strip_prefix('-')
        .or_else(|| line.strip_prefix('\u{2022}'))
        .or_else(|| line.strip_prefix('*'))
    {
        return Some(rest.trim_start().to_string());
    }
    let digits_len = line.chars().take_while(char::is_ascii_digit).count();
    if digits_len == 0 {
        return None;
    }
    let rest = &line[digits_len..];
    let rest = rest.strip_prefix('.').or_else(|| rest.strip_prefix(')'))?;
    Some(rest.trim_start().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_summary_has_no_key_points() {
        assert!(extract_key_points("").is_empty());
    }

    #[test]
    fn plain_paragraphs_are_not_key_points() {
        let summary = "Ceci est un paragraphe normal.\nEt un autre.";
        assert!(extract_key_points(summary).is_empty());
    }

    #[test]
    fn extracts_dash_and_numbered_bullets_and_strips_markers() {
        let summary = "Intro.\n- Premier point\n* Deuxi\u{e8}me point\n2) Troisi\u{e8}me point\n3. Quatri\u{e8}me point";
        let points = extract_key_points(summary);
        assert_eq!(
            points,
            vec![
                "Premier point",
                "Deuxi\u{e8}me point",
                "Troisi\u{e8}me point",
                "Quatri\u{e8}me point"
            ]
        );
    }

    #[test]
    fn caps_at_four_points() {
        let summary = (1..=6)
            .map(|i| format!("- point {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(extract_key_points(&summary).len(), 4);
    }
}
