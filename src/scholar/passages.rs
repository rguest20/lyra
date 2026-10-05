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

pub(super) fn coherent_passages(question: &str, page: &str) -> Vec<String> {
    let query_words = crate::storage::relevant_words(question);
    let mut passages = split_sections(page);
    passages.retain(|passage| !passage.text.trim().is_empty());
    let mut ranked = passages
        .into_iter()
        .filter_map(|passage| {
            let heading_words = crate::storage::relevant_words(&passage.heading_context);
            let body_words = crate::storage::relevant_words(&passage.text);
            let heading_hits = crate::storage::lexical_overlap(&query_words, &heading_words);
            let body_hits = crate::storage::lexical_overlap(&query_words, &body_words);
            let score =
                heading_hits * 4 + body_hits + usize::from(passage.text.starts_with('#')) * 3;
            (score > 0).then_some((score, passage))
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
