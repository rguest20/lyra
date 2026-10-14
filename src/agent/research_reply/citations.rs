//! Resolve only the source labels supplied to the answer generator.
pub(super) fn resolve_source_labels(answer: &str, sources: &[&str]) -> String {
    let mut result = String::with_capacity(answer.len());
    for fragment in answer.split_inclusive(char::is_whitespace) {
        // A legitimate URL can itself contain a source label in its path.
        if fragment.contains("://") {
            result.push_str(fragment);
            continue;
        }
        let mut begin = 0;
        let mut word_start = None;
        for (index, character) in fragment.char_indices().chain([(fragment.len(), ' ')]) {
            if character.is_alphanumeric() || character == '_' {
                word_start.get_or_insert(index);
            } else if let Some(start) = word_start.take() {
                let word = &fragment[start..index];
                let replacement = sources
                    .iter()
                    .enumerate()
                    .find_map(|(source_index, source)| {
                        word.eq_ignore_ascii_case(&format!("source_{}", source_index + 1))
                            .then_some(*source)
                    });
                if let Some(source) = replacement {
                    result.push_str(&fragment[begin..start]);
                    result.push_str(source);
                    begin = index;
                }
            }
        }
        result.push_str(&fragment[begin..]);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_whole_tokens_and_existing_urls_are_preserved() {
        let sources = ["https://example.org/source_1", "https://example.org/second"];
        assert_eq!(
            resolve_source_labels(
                "[Source_1], source_2. source_10 source_01 not_source_1",
                &sources
            ),
            "[https://example.org/source_1], https://example.org/second. source_10 source_01 not_source_1"
        );
        assert_eq!(resolve_source_labels(sources[0], &sources), sources[0]);
        assert_eq!(
            resolve_source_labels("Évidence: SOURCE_2", &sources),
            "Évidence: https://example.org/second"
        );
    }
}
