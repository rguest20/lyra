//! Match a contiguous quote while allowing only differences in whitespace.

/// Return the original passage span, so every stored quote stays byte-exact.
/// Words, punctuation, case, order, and intervening non-whitespace are unchanged.
pub(crate) fn original_span<'a>(passage: &'a str, quote: &str) -> Option<&'a str> {
    if quote.trim().is_empty() {
        return None;
    }
    if let Some(start) = passage.find(quote) {
        return Some(&passage[start..start + quote.len()]);
    }
    let needle = quote.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut normalized = String::new();
    // Each normalized UTF-8 byte maps to the full original character's bounds.
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    let mut whitespace = false;
    for (start, character) in passage.char_indices() {
        let end = start + character.len_utf8();
        if character.is_whitespace() {
            if whitespace {
                bounds.last_mut()?.1 = end;
                continue;
            }
            normalized.push(' ');
            bounds.push((start, end));
            whitespace = true;
        } else {
            normalized.push(character);
            bounds.extend(std::iter::repeat_n((start, end), character.len_utf8()));
            whitespace = false;
        }
    }
    let start = normalized.find(&needle)?;
    let end = start + needle.len() - 1;
    Some(&passage[bounds[start].0..bounds[end].1])
}

#[cfg(test)]
mod tests {
    use super::*;

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
