use crate::transcript::MeetingParticipant;

/// Format the calendar participants as a prompt section, or None when there
/// are none. Participants come before user notes: they are context, notes are
/// authoritative.
pub fn format_participants(participants: &[MeetingParticipant]) -> Option<String> {
    if participants.is_empty() {
        return None;
    }
    let mut lines = String::new();
    for p in participants {
        lines.push_str("- ");
        lines.push_str(&p.name);
        if let Some(email) = p.email.as_deref().filter(|e| !e.is_empty()) {
            lines.push_str(&format!(" <{email}>"));
        }
        if p.is_organizer {
            lines.push_str(" (organizer)");
        }
        if p.is_current_user {
            lines.push_str(" (me)");
        }
        lines.push('\n');
    }
    Some(format!(
        "\n\nMeeting participants (from the calendar invitation; use these \
         names when attributing statements, decisions, and action \
         items):\n---\n{lines}---"
    ))
}

/// Build the user prompt for summarization. Notes the user took during
/// the meeting are appended as their own section so the model can weigh
/// them (decisions, action items, corrections) alongside the transcript.
pub fn build_summarize_prompt(
    transcript_text: &str,
    notes: Option<&str>,
    participants: &[MeetingParticipant],
) -> String {
    let mut prompt = format!("Transcript:\n---\n{transcript_text}\n---");
    if let Some(section) = format_participants(participants) {
        prompt.push_str(&section);
    }
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        prompt.push_str(&format!(
            "\n\nUser notes (taken live during the meeting; treat them as \
             authoritative context for the summary):\n---\n{notes}\n---"
        ));
    }
    prompt
}

/// Build the reduce-stage prompt: the ordered per-chunk summaries plus an
/// explicit whole-meeting, equal-weight instruction (the chunk order is the
/// meeting order, so the model must not over-weight the final chunk).
pub fn build_reduce_prompt(
    part_summaries: &[String],
    notes: Option<&str>,
    participants: &[MeetingParticipant],
) -> String {
    let mut joined = String::new();
    for (i, part) in part_summaries.iter().enumerate() {
        joined.push_str(&format!("=== Part {} ===\n{}\n\n", i + 1, part.trim()));
    }
    let mut prompt = format!(
        "Below are ordered summaries of consecutive parts of ONE meeting \
         (Part 1 = beginning, the last part = end). Merge them into a single \
         summary that covers the whole meeting in order and gives equal weight \
         to every part.\n\nPart summaries:\n---\n{joined}---"
    );
    if let Some(section) = format_participants(participants) {
        prompt.push_str(&section);
    }
    if let Some(notes) = notes.map(str::trim).filter(|n| !n.is_empty()) {
        prompt.push_str(&format!(
            "\n\nUser notes (taken live during the meeting; treat them as \
             authoritative context for the summary):\n---\n{notes}\n---"
        ));
    }
    prompt
}

#[cfg(test)]
mod tests {
    use super::{build_reduce_prompt, build_summarize_prompt, format_participants};
    use crate::transcript::MeetingParticipant;

    #[test]
    fn reduce_prompt_orders_parts_and_demands_equal_weight() {
        let prompt = build_reduce_prompt(&["alpha".into(), "omega".into()], None, &[]);
        assert!(prompt.contains("Part 1"));
        assert!(prompt.contains("Part 2"));
        assert!(prompt.contains("alpha"));
        assert!(prompt.contains("omega"));
        assert!(prompt.contains("equal weight"));
        assert!(prompt.find("alpha").unwrap() < prompt.find("omega").unwrap());
    }

    #[test]
    fn reduce_prompt_appends_notes() {
        let prompt = build_reduce_prompt(&["alpha".into()], Some("decision: ship"), &[]);
        assert!(prompt.contains("User notes"));
        assert!(prompt.contains("decision: ship"));
    }

    #[test]
    fn prompt_without_notes_is_transcript_only() {
        let prompt = build_summarize_prompt("hello world", None, &[]);
        assert!(prompt.contains("Transcript:\n---\nhello world\n---"));
        assert!(!prompt.contains("User notes"));
    }

    #[test]
    fn prompt_includes_user_notes_section() {
        let prompt = build_summarize_prompt("hello", Some("decision: ship friday"), &[]);
        assert!(prompt.contains("User notes"));
        assert!(prompt.contains("decision: ship friday"));
        assert!(prompt.find("Transcript:").unwrap() < prompt.find("User notes").unwrap());
    }

    #[test]
    fn blank_notes_are_ignored() {
        let prompt = build_summarize_prompt("hello", Some("   "), &[]);
        assert!(!prompt.contains("User notes"));
    }

    #[test]
    fn participants_section_renders_markers_and_precedes_notes() {
        let participants = vec![
            MeetingParticipant {
                name: "Alice Martin".to_string(),
                email: Some("alice@corp.com".to_string()),
                is_organizer: true,
                is_current_user: false,
            },
            MeetingParticipant {
                name: "Damien".to_string(),
                email: None,
                is_organizer: false,
                is_current_user: true,
            },
        ];
        let prompt = build_summarize_prompt("hello", Some("ship it"), &participants);
        assert!(prompt.contains("- Alice Martin <alice@corp.com> (organizer)"));
        assert!(prompt.contains("- Damien (me)"));
        assert!(prompt.find("Meeting participants").unwrap() < prompt.find("User notes").unwrap());
        assert!(prompt.find("Transcript:").unwrap() < prompt.find("Meeting participants").unwrap());

        let reduce = build_reduce_prompt(&["alpha".into()], None, &participants);
        assert!(reduce.contains("- Alice Martin <alice@corp.com> (organizer)"));
    }

    #[test]
    fn empty_participants_add_nothing() {
        assert!(format_participants(&[]).is_none());
        let prompt = build_summarize_prompt("hello", None, &[]);
        assert!(!prompt.contains("Meeting participants"));
    }
}
