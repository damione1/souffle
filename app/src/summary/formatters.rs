/// Spoken-command leftovers the LLM sometimes leaves verbatim.
/// Longest phrase first so e.g. "exclamation point" wins over a shorter match.
const SPOKEN_COMMANDS: &[(&str, &str)] = &[
    ("point d'interrogation", "?"),
    ("point d interrogation", "?"),
    ("point d'exclamation", "!"),
    ("point d exclamation", "!"),
    ("nouveau paragraphe", "\n\n"),
    ("exclamation point", "!"),
    ("exclamation mark", "!"),
    ("new paragraph", "\n\n"),
    ("question mark", "?"),
    ("close quote", "\""),
    ("à la ligne", "\n"),
    ("open quote", "\""),
    ("a la ligne", "\n"),
    ("new line", "\n"),
    ("newline", "\n"),
];

/// Conservative post-LLM spoken-command replacements, then squeeze spaces/tabs
/// without collapsing newlines. Do not use the STT whitespace filter here.
pub fn apply_post_polish_formatters(text: &str) -> String {
    let mut commands: Vec<_> = SPOKEN_COMMANDS.to_vec();
    commands.sort_by_key(|(phrase, _)| std::cmp::Reverse(phrase.chars().count()));

    let mut out = text.to_string();
    for (phrase, replacement) in commands {
        out = replace_phrase(&out, phrase, replacement);
    }
    squeeze_horizontal_whitespace(&out)
}

fn replace_phrase(text: &str, phrase: &str, replacement: &str) -> String {
    let hay: Vec<char> = text.chars().collect();
    let needle: Vec<char> = phrase.chars().collect();
    if needle.is_empty() {
        return text.to_string();
    }

    let eat_edges = replacement.contains('\n') || replacement == "?" || replacement == "!";
    let protect_new_line_of = phrase.eq_ignore_ascii_case("new line");

    let mut i = 0;
    let mut out = String::with_capacity(text.len());
    while i < hay.len() {
        if let Some(end) = match_phrase(&hay, i, &needle)
            && is_left_boundary(&hay, i)
            && is_right_boundary(&hay, end)
        {
            if protect_new_line_of && followed_by_of(&hay, end) {
                out.push(hay[i]);
                i += 1;
                continue;
            }

            let mut stop = end;
            if eat_edges {
                while matches!(out.chars().last(), Some(' ' | '\t')) {
                    out.pop();
                }
                while stop < hay.len() && matches!(hay[stop], ' ' | '\t') {
                    stop += 1;
                }
            }
            out.push_str(replacement);
            i = stop;
            continue;
        }
        out.push(hay[i]);
        i += 1;
    }
    out
}

fn match_phrase(hay: &[char], start: usize, needle: &[char]) -> Option<usize> {
    let end = start.checked_add(needle.len())?;
    if end > hay.len() {
        return None;
    }
    let equal = hay[start..end]
        .iter()
        .copied()
        .flat_map(char::to_lowercase)
        .eq(needle.iter().copied().flat_map(char::to_lowercase));
    equal.then_some(end)
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric()
}

fn is_left_boundary(hay: &[char], start: usize) -> bool {
    start == 0 || !is_word_char(hay[start - 1])
}

fn is_right_boundary(hay: &[char], end: usize) -> bool {
    end == hay.len() || !is_word_char(hay[end])
}

fn followed_by_of(hay: &[char], end: usize) -> bool {
    let mut i = end;
    let mut saw_space = false;
    while i < hay.len() && matches!(hay[i], ' ' | '\t') {
        saw_space = true;
        i += 1;
    }
    if !saw_space || i + 2 > hay.len() {
        return false;
    }
    if !hay[i].eq_ignore_ascii_case(&'o') || !hay[i + 1].eq_ignore_ascii_case(&'f') {
        return false;
    }
    is_right_boundary(hay, i + 2)
}

fn squeeze_horizontal_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            result.push('\n');
        }
        result.push_str(&squeeze_line(line));
    }
    result
}

fn squeeze_line(line: &str) -> String {
    let mut squeezed = String::with_capacity(line.len());
    let mut last_was_space = false;
    for ch in line.chars() {
        if ch == ' ' || ch == '\t' {
            if !last_was_space {
                squeezed.push(' ');
                last_was_space = true;
            }
        } else {
            squeezed.push(ch);
            last_was_space = false;
        }
    }
    squeezed.trim_end_matches([' ', '\t']).to_string()
}

#[cfg(test)]
mod tests {
    use super::{SPOKEN_COMMANDS, apply_post_polish_formatters};

    #[test]
    fn spoken_commands_are_listed_longest_first() {
        let mut sorted = SPOKEN_COMMANDS.to_vec();
        sorted.sort_by_key(|(phrase, _)| std::cmp::Reverse(phrase.chars().count()));
        assert_eq!(SPOKEN_COMMANDS, sorted.as_slice());
    }

    #[test]
    fn spoken_command_replacements() {
        let cases = [
            ("hello new line world", "hello\nworld"),
            ("hello newline world", "hello\nworld"),
            ("hello NEW LINE world", "hello\nworld"),
            ("hello new paragraph world", "hello\n\nworld"),
            ("are you sure question mark", "are you sure?"),
            ("wow exclamation mark", "wow!"),
            ("wow exclamation point", "wow!"),
            ("open quote hello close quote", "\" hello \""),
            ("suite à la ligne merci", "suite\nmerci"),
            ("suite a la ligne merci", "suite\nmerci"),
            ("idée nouveau paragraphe suite", "idée\n\nsuite"),
            ("vrai point d'interrogation", "vrai?"),
            ("vrai point d interrogation", "vrai?"),
            ("bravo point d'exclamation", "bravo!"),
            ("bravo point d exclamation", "bravo!"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                apply_post_polish_formatters(input),
                expected,
                "input: {input:?}"
            );
        }
    }

    #[test]
    fn does_not_eat_new_line_of_credit() {
        assert_eq!(
            apply_post_polish_formatters("I need a new line of credit"),
            "I need a new line of credit"
        );
    }

    #[test]
    fn does_not_replace_standalone_french_point() {
        assert_eq!(
            apply_post_polish_formatters("C'est le point important"),
            "C'est le point important"
        );
    }

    #[test]
    fn squeezes_spaces_and_tabs_but_keeps_newlines() {
        assert_eq!(
            apply_post_polish_formatters("hello   \t  world\n\nnext\t  line  "),
            "hello world\n\nnext line"
        );
    }

    #[test]
    fn trims_trailing_spaces_on_lines() {
        assert_eq!(
            apply_post_polish_formatters("hello   \nworld\t"),
            "hello\nworld"
        );
    }
}
