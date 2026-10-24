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

pub(super) fn append_source_list(answer: &mut String, sources: &[&str], label: &str) {
    let mut unique_sources = Vec::new();
    for source in sources {
        if !unique_sources.contains(source) {
            unique_sources.push(*source);
        }
    }
    if !unique_sources.is_empty() && !unique_sources.iter().any(|source| answer.contains(source)) {
        answer.push_str(&format!("\n\n{label}: {}", unique_sources.join(", ")));
    }
}

pub(super) fn mask_source_urls(answer: &str, sources: &[&str]) -> String {
    sources
        .iter()
        .enumerate()
        .fold(answer.to_owned(), |masked, (index, source)| {
            masked.replace(source, &format!("source_{}", index + 1))
        })
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

    #[test]
    fn appends_sources_once_and_keeps_the_requested_label() {
        let sources = ["https://example.org/one", "https://example.org/two"];
        let mut answer = "Supported answer.".to_owned();
        append_source_list(&mut answer, &sources, "Sources");
        assert_eq!(
            answer,
            "Supported answer.\n\nSources: https://example.org/one, https://example.org/two"
        );
        append_source_list(&mut answer, &sources, "Sources");
        assert_eq!(answer.matches("Sources:").count(), 1);
        let mut checked = "Useful partial.".to_owned();
        append_source_list(&mut checked, &sources, "Sources checked");
        assert!(
            checked.ends_with("Sources checked: https://example.org/one, https://example.org/two")
        );
        let mut repeated = "Useful partial.".to_owned();
        append_source_list(
            &mut repeated,
            &[sources[0], sources[0], sources[1]],
            "Sources",
        );
        assert!(repeated.ends_with("Sources: https://example.org/one, https://example.org/two"));
    }

    #[test]
    fn masks_only_the_supplied_urls_with_their_evidence_labels() {
        let sources = ["https://example.org/one", "https://example.org/two"];
        assert_eq!(
            mask_source_urls(
                "One account (https://example.org/one), then another.",
                &sources
            ),
            "One account (source_1), then another."
        );
    }
}
