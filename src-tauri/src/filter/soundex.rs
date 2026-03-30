/// Standard American Soundex encoding.
/// Returns a 4-character code: first letter + 3 digits.
/// H and W are transparent — they don't separate identical codes.
pub fn soundex(word: &str) -> Option<String> {
    let chars: Vec<char> = word
        .chars()
        .filter(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_uppercase())
        .collect();

    let first = *chars.first()?;
    let mut code = String::with_capacity(4);
    code.push(first);

    let mut last_digit = letter_to_digit(first);

    for &ch in &chars[1..] {
        let digit = letter_to_digit(ch);
        if digit == '0' {
            // H and W (mapped to '0') are transparent — don't update last_digit
            // A, E, I, O, U also map to '0' but DO separate identical codes
            if ch != 'H' && ch != 'W' {
                last_digit = '0';
            }
            continue;
        }
        if digit != last_digit {
            code.push(digit);
            if code.len() == 4 {
                return Some(code);
            }
        }
        last_digit = digit;
    }

    while code.len() < 4 {
        code.push('0');
    }
    Some(code)
}

fn letter_to_digit(ch: char) -> char {
    match ch.to_ascii_uppercase() {
        'B' | 'F' | 'P' | 'V' => '1',
        'C' | 'G' | 'J' | 'K' | 'Q' | 'S' | 'X' | 'Z' => '2',
        'D' | 'T' => '3',
        'L' => '4',
        'M' | 'N' => '5',
        'R' => '6',
        _ => '0', // A, E, I, O, U, H, W
    }
}

#[cfg(test)]
mod tests {
    use super::soundex;

    #[test]
    fn classic_examples() {
        assert_eq!(soundex("Robert"), Some("R163".into()));
        assert_eq!(soundex("Rupert"), Some("R163".into()));
        assert_eq!(soundex("Ashcraft"), Some("A261".into()));
        assert_eq!(soundex("Tymczak"), Some("T522".into()));
    }

    #[test]
    fn short_words_padded() {
        assert_eq!(soundex("Al"), Some("A400".into()));
        assert_eq!(soundex("I"), Some("I000".into()));
    }

    #[test]
    fn empty_returns_none() {
        assert_eq!(soundex(""), None);
        assert_eq!(soundex("123"), None);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(soundex("smith"), soundex("SMITH"));
        assert_eq!(soundex("smith"), soundex("Smith"));
    }
}
