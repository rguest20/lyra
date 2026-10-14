//! Match contiguous quotes with lossless spacing/internal apostrophe recovery.

/// Return the original passage span, so every stored quote stays byte-exact.
/// Words, case, order and punctuation stay unchanged except typographic
/// apostrophes between letters. Quote delimiters and prime marks stay exact.
pub(crate) fn original_span<'a>(passage: &'a str, quote: &str) -> Option<&'a str> {
    if quote.trim().is_empty() {
        return None;
    }
    if let Some(start) = passage.find(quote) {
        return Some(&passage[start..start + quote.len()]);
    }
    let needle = normalized(quote).0.trim().to_owned();
    let (normalized, bounds) = normalized(passage);
    let start = normalized.find(&needle)?;
    let end = start + needle.len() - 1;
    Some(&passage[bounds[start].0..bounds[end].1])
}

fn normalized(passage: &str) -> (String, Vec<(usize, usize)>) {
    let mut normalized = String::new();
    // Each normalized UTF-8 byte maps to the full original character's bounds.
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut whitespace = false;
    let mut characters = passage.char_indices().peekable();
    let mut previous: Option<char> = None;
    while let Some((start, character)) = characters.next() {
        let end = start + character.len_utf8();
        if character.is_whitespace() {
            if whitespace {
                bounds.last_mut().unwrap().1 = end;
                previous = Some(character);
                continue;
            }
            normalized.push(' ');
            bounds.push((start, end));
            whitespace = true;
        } else {
            let canonical = if matches!(character, '’' | '‘')
                && previous.is_some_and(char::is_alphabetic)
                && characters
                    .peek()
                    .is_some_and(|(_, next)| next.is_alphabetic())
            {
                '\''
            } else {
                character
            };
            normalized.push(canonical);
            bounds.extend(std::iter::repeat_n((start, end), canonical.len_utf8()));
            whitespace = false;
        }
        previous = Some(character);
    }
    (normalized, bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_internal_apostrophe_typography_without_changing_original_bytes() {
        let passage = "Before. Café’s telescope sees Orion’s sword. After.";
        assert_eq!(
            original_span(passage, "Café's telescope sees Orion's sword."),
            Some("Café’s telescope sees Orion’s sword.")
        );
        assert!(original_span("The value is f′(x).", "The value is f'(x).").is_none());
        assert!(original_span("The height is 5’6.", "The height is 5'6.").is_none());
        assert!(original_span("The token is “name”.", "The token is \"name\".").is_none());
    }

    #[test]
    fn restores_original_newlines_tabs_and_unicode_spaces() {
        let passage = "Before. Café\u{a0}chicken\t soup.\r\n\nIt is served warm. After.";
        let quote = "Café chicken soup. It is served warm.";
        assert_eq!(
            original_span(passage, quote),
            Some("Café\u{a0}chicken\t soup.\r\n\nIt is served warm.")
        );
    }

    #[test]
    fn does_not_change_words_negation_punctuation_or_case_or_stitch_spans() {
        let passage =
            "Chicken soup is not curry.\nCurry is a different dish.\nSoup is served warm.";
        for quote in [
            "Chicken soup is curry.",
            "chicken soup is not curry.",
            "Chicken soup is not curry!",
            "Chicken soup is not curry. Soup is served warm.",
            "Chicken soup is not curry. Soup is served warm. Invented ending.",
            "",
            " \n ",
        ] {
            assert!(original_span(passage, quote).is_none(), "{quote:?}");
        }
        assert_eq!(
            original_span(passage, "Chicken soup is not curry."),
            Some("Chicken soup is not curry.")
        );
    }
}
