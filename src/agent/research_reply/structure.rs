//! Conservative checks for source structure that a semantic reviewer can miss.
//! These require a faithful rewrite; they do not infer aliases or list items.
use super::normalized_words;
use serde_json::Value;

// Catch changed source names and unseen proper names inside sentences. Sentence
// openings remain with semantic review because ordinary prose is capitalized.
// This is not a complete invented-name or relationship detector.
pub(super) fn unsupported_entity_name(answer: &str, evidence: &[Value]) -> Option<String> {
    let passages = evidence
        .iter()
        .filter_map(|e| e["passage"].as_str())
        .map(normalized_words)
        .collect::<Vec<_>>();
    crate::entities::literal_name_phrases(answer)
        .into_iter()
        .find(|name| {
            let words = normalized_words(name);
            !words.is_empty()
                && ((words.len() >= 2
                    && passages.iter().any(|passage| passage.contains(&words[0])))
                    || phrase_is_mid_sentence(answer, name))
                && !passages
                    .iter()
                    .any(|passage| passage.windows(words.len()).any(|window| window == words))
        })
}

fn phrase_is_mid_sentence(answer: &str, name: &str) -> bool {
    answer.match_indices(name).any(|(index, _)| {
        answer[..index]
            .trim_end_matches([' ', '\t', '"', '\'', '“', '”', '‘', '’', '*', '('])
            .chars()
            .next_back()
            .is_some_and(|previous| !matches!(previous, '.' | '!' | '?' | ':' | ';' | '\n'))
    })
}

// Without source-defined boundaries, turning "X and Y" into separate list
// items can change a compound name. Even when X/Y really are separate people,
// preserving the conjunction is a useful answer and avoids guessing boundaries.
pub(super) fn altered_source_conjunction(answer: &str, evidence: &[Value]) -> Option<String> {
    let sources = evidence
        .iter()
        .filter_map(|entry| entry["passage"].as_str())
        .map(|passage| (passage, word_spans(passage)))
        .collect::<Vec<_>>();
    let answer_words = word_spans(answer);
    for (passage, words) in &sources {
        for phrase in words.windows(3).filter(|phrase| phrase[1].0 == "and") {
            let left = &phrase[0].0;
            let right = &phrase[2].0;
            if list_boundary(&passage[phrase[0].2..phrase[2].1]) {
                continue;
            }
            let newly_split = answer_words.windows(2).any(|pair| {
                pair[0].0 == *left
                    && pair[1].0 == *right
                    && list_boundary(&answer[pair[0].2..pair[1].1])
            });
            let explicitly_separated = sources.iter().any(|(text, tokens)| {
                tokens.windows(2).any(|pair| {
                    pair[0].0 == *left
                        && pair[1].0 == *right
                        && list_boundary(&text[pair[0].2..pair[1].1])
                })
            });
            if newly_split && !explicitly_separated {
                return Some(passage[phrase[0].1..phrase[2].2].to_owned());
            }
        }
    }
    None
}

fn list_boundary(separator: &str) -> bool {
    separator.contains([',', ';', '\n'])
}

// Detect the precise, source-literal strengthening "A; B" -> "A because B".
// Three matching words on each side keep this conservative. Paraphrased or
// implicit causal claims still need semantic review; this is not a parser.
pub(super) fn unsupported_causal_join(answer: &str, evidence: &[Value]) -> Option<String> {
    let words = word_spans(answer);
    for connector in ["because", "therefore", "thus", "consequently", "so"] {
        for (index, _) in words
            .iter()
            .enumerate()
            .filter(|(_, word)| word.0 == connector)
        {
            if index < 3 || index + 4 > words.len() {
                continue;
            }
            for passage in evidence
                .iter()
                .filter_map(|entry| entry["passage"].as_str())
            {
                let source_words = word_spans(passage);
                for window in source_words.windows(6) {
                    let before_matches = words[index - 3..index]
                        .iter()
                        .zip(&window[..3])
                        .all(|(left, right)| left.0 == right.0);
                    let after_matches = words[index + 1..index + 4]
                        .iter()
                        .zip(&window[3..])
                        .all(|(left, right)| left.0 == right.0);
                    if before_matches
                        && after_matches
                        && passage[window[2].2..window[3].1].contains(['.', ';', '\n'])
                    {
                        return Some(connector.into());
                    }
                }
            }
        }
    }
    None
}

// Naming an actor in a passage does not make that actor its reporting source.
// Permit named attribution only when the quoted words identify that speaker.
pub(super) fn unsupported_named_attribution(answer: &str, evidence: &[Value]) -> Option<String> {
    let answer_words = word_spans(answer);
    let speech = [
        "said",
        "says",
        "reported",
        "reports",
        "stated",
        "states",
        "wrote",
        "writes",
        "explained",
        "suggested",
        "claimed",
    ];
    for name in crate::entities::literal_name_phrases(answer) {
        let name_words = normalized_words(&name);
        if name_words.is_empty() {
            continue;
        }
        let is_attribution =
            answer_words
                .windows(name_words.len())
                .enumerate()
                .any(|(index, window)| {
                    if !window
                        .iter()
                        .zip(&name_words)
                        .all(|(word, name)| word.0 == *name)
                    {
                        return false;
                    }
                    let before = &answer_words[..index];
                    let prefix = before.last().is_some_and(|word| {
                        !answer[word.2..window[0].1].contains([',', '.', ';', ':', '\n'])
                            && (word.0 == "source"
                                || (word.0 == "to"
                                    && before.len() >= 2
                                    && matches!(
                                        before[before.len() - 2].0.as_str(),
                                        "according" | "attributed"
                                    )))
                    });
                    let speech_after = answer_words
                        .get(index + name_words.len())
                        .is_some_and(|word| speech.contains(&word.0.as_str()));
                    prefix || speech_after
                });
        if !is_attribution {
            continue;
        }
        let named_in_source = evidence
            .iter()
            .filter_map(|entry| entry["passage"].as_str())
            .any(|passage| {
                let words = normalized_words(passage);
                words
                    .windows(name_words.len())
                    .enumerate()
                    .any(|(index, window)| {
                        if window != name_words {
                            return false;
                        }
                        let prefix = &words[..index];
                        prefix.ends_with(&["according".into(), "to".into()])
                            || prefix.ends_with(&["report".into(), "by".into()])
                            || prefix.ends_with(&["statement".into(), "by".into()])
                            || words
                                .get(index + name_words.len())
                                .is_some_and(|word| speech.contains(&word.as_str()))
                    })
            });
        if !named_in_source {
            return Some(name);
        }
    }
    None
}

fn word_spans(text: &str) -> Vec<(String, usize, usize)> {
    let mut words = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices().chain([(text.len(), ' ')]) {
        if character.is_alphanumeric() {
            start.get_or_insert(index);
        } else if let Some(begin) = start.take() {
            words.push((text[begin..index].to_lowercase(), begin, index));
        }
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn ordinary_uncertainty_contractions_are_not_invented_names() {
        let evidence = vec![json!({"passage":"Ada Voss founded Meridian Observatory."})];
        for draft in [
            "Ada Voss founded Meridian Observatory, but I'm unsure about other details.",
            "Ada Voss founded Meridian Observatory, but I’m unsure about other details.",
        ] {
            assert!(unsupported_entity_name(draft, &evidence).is_none());
        }
    }

    #[test]
    fn causal_join_checks_preserve_explicit_source_causes_and_evidence_limits() {
        let passage = "The laboratory closed because the building flooded.";
        assert!(unsupported_causal_join(passage, &[json!({"passage":passage})]).is_none());
        let source = json!({"passage":"The laboratory measured magnetic susceptibility."});
        assert!(
            unsupported_causal_join(
                "I can't establish the outcome because the passage does not report it.",
                &[source]
            )
            .is_none()
        );
        let passage = "The publisher printed three books. The library acquired two copies.";
        assert_eq!(
            unsupported_causal_join(
                "The publisher printed three books, therefore the library acquired two copies.",
                &[json!({"passage":passage})]
            )
            .as_deref(),
            Some("therefore")
        );
    }

    #[test]
    fn named_attribution_requires_a_speaker_not_just_a_mentioned_actor() {
        let evidence = vec![json!({"passage":"Ada Voss founded Meridian Observatory."})];
        assert_eq!(
            unsupported_named_attribution(
                "According to Ada Voss, Meridian Observatory opened.",
                &evidence
            )
            .as_deref(),
            Some("Ada Voss")
        );
        assert!(
            unsupported_named_attribution(
                "According to the source, Ada Voss founded Meridian Observatory.",
                &evidence
            )
            .is_none()
        );
        let evidence =
            vec![json!({"passage":"Ada Voss said Meridian Observatory opened in 1974."})];
        assert!(
            unsupported_named_attribution(
                "According to Ada Voss, Meridian Observatory opened in 1974.",
                &evidence
            )
            .is_none()
        );
    }

    #[test]
    fn explicit_boundaries_are_allowed_and_unicode_spans_are_safe() {
        let evidence = vec![json!({"passage":"Müller and Şahin founded it."})];
        assert_eq!(
            altered_source_conjunction("Müller, Şahin founded it.", &evidence).as_deref(),
            Some("Müller and Şahin")
        );
        assert!(altered_source_conjunction("Müller and Şahin founded it.", &evidence).is_none());
        let evidence = vec![json!({"passage":"Founders: Müller, Şahin, and Vale."})];
        assert!(altered_source_conjunction("Müller, Şahin, Vale founded it.", &evidence).is_none());
    }

    #[test]
    fn bullet_boundaries_and_alternative_source_punctuation() {
        let first = json!({"passage":"The works include Aster and Ash and Sea and Silence."});
        assert!(
            altered_source_conjunction("- Aster\n- Ash\n- Sea and Silence", &[first.clone()])
                .is_some()
        );
        let second = json!({"passage":"The titles are Aster, Ash, Sea, Silence."});
        assert!(altered_source_conjunction("Aster, Ash, Sea, Silence", &[first, second]).is_none());
    }
}
