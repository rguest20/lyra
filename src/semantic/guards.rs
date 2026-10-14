//! Conservative source-role and stance checks; never positive factual proof.
use super::*;

// A list member cannot inherit the containing list's predicate. Only reject
// the high-precision case where every literal subject mention follows every
// forward listing predicate. Inverse paraphrases and unresolved pronouns stay
// with the semantic reviewer rather than being guessed by this check.
pub(super) fn list_role_conflict(
    finding: &ScholarFinding,
    evidence: &str,
) -> Option<(&'static str, &'static str)> {
    let relation = finding.relation.to_lowercase();
    if !matches!(
        relation.split_whitespace().next(),
        Some("include" | "includes" | "list" | "lists")
    ) {
        return None;
    }
    let text = evidence.to_lowercase();
    let subject = finding.subject.to_lowercase();
    let words = |value: &str| {
        value
            .split(|ch: char| !ch.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let text = words(&text);
    let subject = words(&subject);
    if subject.is_empty() {
        return None;
    }
    let last_predicate = text.iter().rposition(|word| {
        matches!(
            word.as_str(),
            "include" | "includes" | "including" | "list" | "lists"
        )
    })?;
    let positions = text
        .windows(subject.len())
        .enumerate()
        .filter_map(|(index, window)| (window == subject).then_some(index))
        .collect::<Vec<_>>();
    (!positions.is_empty() && positions.iter().all(|index| *index > last_predicate)).then_some((
        "insufficient",
        "The candidate turns a listed member into the containing category; the source does not assign it the listing relationship.",
    ))
}

// Conservative checks for stance errors observed in live small-model probes.
// These only restrict approval; they never establish positive factual support.
pub(super) fn stance_conflict(
    finding: &ScholarFinding,
    evidence: &str,
) -> Option<(&'static str, &'static str)> {
    let normalize = |value: &str| {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_lowercase()
    };
    let claim = normalize(&format!(
        "{} {} {}",
        finding.subject, finding.relation, finding.object
    ));
    let source = normalize(evidence);
    let has = |text: &str, phrases: &[&str]| {
        phrases.iter().any(|phrase| {
            if phrase.trim().contains(' ') {
                text.contains(phrase.trim())
            } else {
                text.split(|ch: char| !ch.is_alphanumeric()).any(|word| {
                    if matches!(
                        *phrase,
                        "may" | "might" | "not " | "false" | "idea" | "true"
                    ) {
                        word == phrase.trim()
                    } else {
                        word.starts_with(phrase.trim())
                    }
                })
            }
        })
    };
    let uncertainty = [
        "has been suggested",
        "is suggested",
        "may have",
        "might have",
        "possibly",
        "allegedly",
        "is alleged",
        "reportedly",
        "it is believed",
        "is thought to",
        "hypothesized",
        "hypothesised",
    ];
    let preserves_uncertainty = [
        "suggest",
        "may",
        "might",
        "possib",
        "alleg",
        "report",
        "believ",
        "thought",
        "hypothe",
        "according to",
        "claimed",
        "propos",
        "theor",
        "uncertain",
    ];
    if has(&source, &uncertainty) && !has(&claim, &preserves_uncertainty) {
        return Some((
            "qualified",
            "Source uncertainty or attribution is absent from the candidate; keep the suggestion or allegation qualified.",
        ));
    }
    let disproof = [
        "disproven",
        "disproved",
        "debunked",
        "refuted",
        "scientifically false",
    ];
    let preserves_disproof = [
        "disproven",
        "disproved",
        "debunk",
        "refut",
        "false",
        "incorrect",
        "untrue",
        "invalid",
        "not ",
    ];
    let words = claim
        .split(|ch: char| !ch.is_alphanumeric())
        .collect::<Vec<_>>();
    let endorses = words.iter().any(|word| {
        matches!(
            *word,
            "true" | "correct" | "valid" | "accurate" | "fact" | "factual" | "verified"
        )
    });
    let presents_disproved_idea_as_fact = !has(
        &claim,
        &[
            "conception",
            "idea",
            "belief",
            "hypothesis",
            "theory",
            "claim",
            "suggest",
            "alleg",
        ],
    );
    if has(&source, &disproof)
        && !has(&claim, &preserves_disproof)
        && (endorses || presents_disproved_idea_as_fact)
    {
        return Some((
            "contradicted",
            "The source disproves the proposition; the candidate presents it as factual without that disproof.",
        ));
    }
    if has(
        &source,
        &[
            "not exactly",
            "not perfectly",
            "approximately",
            "roughly",
            "nearly",
            "almost",
        ],
    ) && has(
        &claim,
        &["perfectly", "perfect sphere", "exactly", "precisely"],
    ) && !has(&claim, &["not ", "approx", "rough", "nearly", "almost"])
    {
        return Some((
            "qualified",
            "The source describes an approximation; the candidate claims exactness.",
        ));
    }
    None
}

pub(super) fn plural_claim_without_plural_evidence(
    finding: &ScholarFinding,
    evidence: &str,
) -> bool {
    let claim = format!(
        "{} {} {}",
        finding.subject, finding.relation, finding.object
    )
    .to_ascii_lowercase();
    let source = evidence.to_ascii_lowercase();
    let plurality = [
        "variety of",
        "range of",
        "various",
        "multiple",
        "several",
        "numerous",
        "different",
        "both",
    ];
    if !plurality.iter().any(|marker| claim.contains(marker)) {
        return false;
    }
    if plurality.iter().any(|marker| source.contains(marker)) {
        return false;
    }
    let explicit_list = (source.contains(',')
        && (source.contains(" and ") || source.contains(" or ")))
        || source
            .lines()
            .filter(|line| {
                let line = line.trim_start();
                line.starts_with("- ") || line.starts_with("* ") || line.starts_with("• ")
            })
            .take(2)
            .count()
            >= 2;
    !explicit_list
}

#[cfg(test)]
mod tests;
