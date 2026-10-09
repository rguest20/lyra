//! Select small, contiguous page sections for external-source extraction.

const MAX_PASSAGES: usize = 2;
const MAX_PASSAGE_CHARS: usize = 2_200;

#[derive(Debug)]
struct Passage {
    text: String,
    heading_context: String,
    has_list: bool,
    order: usize,
    section_id: usize,
}

/// Reject website controls and footer bundles, not ordinary mentions of cookies
/// or privacy in article prose. Also applied to findings saved before this guard.
pub(crate) fn is_site_chrome(text: &str) -> bool {
    let lower = text.to_lowercase();
    let lines = lower
        .lines()
        .map(|line| line.trim().trim_start_matches(['*', '-', '#', '>']).trim())
        .collect::<Vec<_>>();
    let consent_controls = lines.iter().any(|line| {
        matches!(
            *line,
            "i accept" | "i do not accept" | "accept all cookies" | "reject all cookies"
        )
    });
    let consent_text = [
        "cookie choices",
        "consenting to",
        "use certain cookies",
        "cookie preferences",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    let footer_markers = [
        "privacy policy",
        "do not sell my info",
        "terms of use",
        "all rights reserved",
        "cookie choices",
        "popular research",
    ]
    .iter()
    .filter(|marker| lower.contains(**marker))
    .count();
    (consent_controls && consent_text) || footer_markers >= 3
}

pub(super) fn coherent_passages(question: &str, page: &str) -> Vec<String> {
    let query_words = crate::storage::relevant_words(question);
    let mut passages = split_sections(page);
    passages.retain(|passage| !passage.text.trim().is_empty() && !is_site_chrome(&passage.text));
    let mut ranked = passages
        .into_iter()
        .filter_map(|passage| {
            let heading_words = crate::storage::relevant_words(&passage.heading_context);
            let body_words = crate::storage::relevant_words(&passage.text);
            let heading_hits = crate::storage::lexical_overlap(&query_words, &heading_words);
            let body_hits = crate::storage::lexical_overlap(&query_words, &body_words);
            let score =
                heading_hits * 4 + body_hits + usize::from(passage.text.starts_with('#')) * 3;
            // An inherited article title must not make unrelated navigation relevant.
            (body_hits > 0).then_some((score, passage))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.order.cmp(&right.order))
    });
    if ranked.is_empty() {
        return Vec::new();
    }
    let first = ranked.remove(0);
    let mut selected = vec![first.1];
    if let Some(diverse) = ranked.iter().position(|(score, passage)| {
        passage.section_id != selected[0].section_id
            && passage.has_list != selected[0].has_list
            && *score * 2 >= first.0
    }) {
        selected.push(ranked.remove(diverse).1);
    } else if let Some(distinct) = ranked
        .iter()
        .position(|(_, passage)| passage.section_id != selected[0].section_id)
    {
        selected.push(ranked.remove(distinct).1);
    } else if let Some((_, passage)) = ranked.into_iter().next() {
        selected.push(passage);
    }
    selected
        .into_iter()
        .take(MAX_PASSAGES)
        .map(|passage| passage.text)
        .collect()
}

fn split_sections(page: &str) -> Vec<Passage> {
    let mut passages = Vec::new();
    let mut heading_stack: Vec<(usize, String)> = Vec::new();
    let mut section = String::new();
    let mut heading_context = String::new();
    let mut section_id = 0;
    for line in page.lines() {
        if let Some((level, heading)) = parse_heading(line) {
            push_section(&mut passages, &section, &heading_context, section_id);
            section.clear();
            section_id += 1;
            while heading_stack
                .last()
                .is_some_and(|(depth, _)| *depth >= level)
            {
                heading_stack.pop();
            }
            heading_stack.push((level, heading.to_owned()));
            heading_context = heading_stack
                .iter()
                .map(|(_, heading)| heading.as_str())
                .collect::<Vec<_>>()
                .join(" ");
        }
        section.push_str(line);
        section.push('\n');
    }
    push_section(&mut passages, &section, &heading_context, section_id);
    passages
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    let level = trimmed.bytes().take_while(|byte| *byte == b'#').count();
    if !(1..=6).contains(&level)
        || !trimmed
            .as_bytes()
            .get(level)
            .is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    let heading = trimmed[level..].trim();
    (!heading.is_empty()).then_some((level, heading))
}

fn push_section(
    passages: &mut Vec<Passage>,
    section: &str,
    heading_context: &str,
    section_id: usize,
) {
    let section = section.trim();
    if section.is_empty() {
        return;
    }
    let mut chunk = String::new();
    for line in section.split_inclusive('\n') {
        if !chunk.is_empty() && chunk.chars().count() + line.chars().count() > MAX_PASSAGE_CHARS {
            push_chunk(passages, &chunk, heading_context, section_id);
            chunk.clear();
        }
        if line.chars().count() > MAX_PASSAGE_CHARS {
            for character in line.chars() {
                if chunk.chars().count() >= MAX_PASSAGE_CHARS {
                    push_chunk(passages, &chunk, heading_context, section_id);
                    chunk.clear();
                }
                chunk.push(character);
            }
        } else {
            chunk.push_str(line);
        }
    }
    push_chunk(passages, &chunk, heading_context, section_id);
}

fn push_chunk(passages: &mut Vec<Passage>, chunk: &str, heading_context: &str, section_id: usize) {
    let text = chunk.trim().to_owned();
    if text.is_empty() {
        return;
    }
    let list_lines = text
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            ["* ", "- ", "• "]
                .iter()
                .any(|prefix| line.starts_with(prefix))
        })
        .count();
    passages.push(Passage {
        text,
        heading_context: heading_context.to_owned(),
        has_list: list_lines >= 2,
        order: passages.len(),
        section_id,
    });
}

#[cfg(test)]
mod tests {
    use super::coherent_passages;

    #[test]
    fn cofresh_footer_is_not_an_article_passage_even_with_query_word_overlap() {
        let footer = include_str!("../../tests/fixtures/cofresh-footer.txt");
        assert!(super::is_site_chrome(footer));
        assert!(coherent_passages("Research crisp flavours", footer).is_empty());
        assert!(!super::is_site_chrome(
            "HTTP cookies store small pieces of data. A privacy policy describes their use."
        ));
        let article = format!("# Cofresh\nCofresh makes savoury snacks.\n\n## Footer\n{footer}");
        let selected = coherent_passages("Cofresh snacks research", &article);
        assert_eq!(selected.len(), 1);
        assert!(selected[0].contains("savoury snacks"));
        assert!(!selected[0].contains("I ACCEPT"));
    }

    #[test]
    fn inherited_article_title_does_not_select_related_navigation() {
        let page = "# Antidisestablishmentarianism\nAntidisestablishmentarianism is a political position.\n\n### Related\n* News\n* The USD Vista\n";
        let passages = coherent_passages(
            "What is the political ideology of antidisestablishmentarianism?",
            page,
        );
        assert_eq!(passages.len(), 1);
        assert!(passages[0].contains("political position"));
        assert!(!passages[0].contains("Related"));
    }

    #[test]
    fn keeps_a_list_and_a_prose_section_as_separate_contiguous_passages() {
        let page = "# Chicken Soup\nChicken soup uses chicken stock with carrots and celery.\n\n## Ingredients\n* Chicken\n* Carrots\n* Celery\n\n## Newsletter\nSubscribe for updates.\n";
        let passages = coherent_passages("What ingredients are in chicken soup?", page);
        assert_eq!(passages.len(), 2);
        assert!(
            passages
                .iter()
                .any(|passage| passage.contains("* Chicken\n* Carrots\n* Celery"))
        );
        assert!(
            passages
                .iter()
                .any(|passage| passage.contains("chicken stock with carrots and celery"))
        );
        assert!(!passages.iter().any(|passage| passage.contains("Subscribe")));
        assert!(passages.iter().all(|passage| page.contains(passage)));
    }
}
