//! Evidence decisions for a proposed Scholar claim. A search hit or a model
//! extraction score is not itself verification.

use super::*;

pub(super) struct CorroborationCheck {
    pub independent_sources: Vec<(i64, String, String, String)>,
    pub possible_copy_of: Option<i64>,
    pub shared_source_with: Option<i64>,
    pub provenance_link: Option<(i64, String)>,
}

pub(super) fn check_corroboration(
    tx: &Transaction<'_>,
    finding: &ScholarFinding,
    source_excerpt: &str,
    source_url: &str,
    current_job_id: &str,
    current_source_id: Option<&str>,
) -> io::Result<CorroborationCheck> {
    let current_provenance = page_provenance(tx, current_job_id, source_url)?;
    let mut statement = tx
        .prepare(
            "SELECT f.id, f.job_id, f.source_url, f.source_excerpt,
                    a.independence_key, a.independence_verified,
                    a.source_kind, a.direct_support, f.confidence,
                    (SELECT c.page_provenance_json FROM scholar_search_candidates c
                     WHERE c.job_id = f.job_id AND c.fetched_url = f.source_url
                     ORDER BY c.id DESC LIMIT 1)
         FROM scholar_findings f
         JOIN scholar_source_assessments a ON a.finding_id = f.id
         WHERE (lower(trim(f.subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(f.subject)) AND e.right_name = lower(trim(?1))))
           AND lower(trim(f.relation)) = lower(trim(?2))
           AND lower(trim(f.object)) = lower(trim(?3))
           AND f.valid_from IS ?4 AND f.valid_until IS ?5
           AND f.status IN ('needs_review', 'promoted')
         ORDER BY f.id DESC LIMIT 50",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map(
            params![
                finding.subject,
                finding.relation,
                finding.object,
                finding.valid_from,
                finding.valid_until,
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<i64>>(7)? == Some(1),
                    row.get::<_, f64>(8)?,
                    row.get::<_, Option<String>>(9)?,
                ))
            },
        )
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    let mut check = CorroborationCheck {
        independent_sources: Vec::new(),
        possible_copy_of: None,
        shared_source_with: None,
        provenance_link: None,
    };
    for (
        id,
        job_id,
        url,
        earlier_excerpt,
        earlier_key,
        verified,
        kind,
        direct,
        confidence,
        earlier_provenance_json,
    ) in rows
    {
        if url == source_url
            || current_source_id.is_some_and(|key| earlier_key.as_deref() == Some(key))
        {
            check.shared_source_with.get_or_insert(id);
            continue;
        }
        let earlier_provenance = parse_page_provenance(earlier_provenance_json.as_deref());
        if let Some(reason) = provenance_link(
            source_url,
            current_provenance.as_ref(),
            &url,
            earlier_provenance.as_ref(),
        ) {
            check.provenance_link.get_or_insert((id, reason));
            continue;
        }
        if substantial_passage_overlap(source_excerpt, &earlier_excerpt) {
            check.possible_copy_of.get_or_insert(id);
            continue;
        }
        if current_source_id.is_some()
            && verified
            && kind == "secondary"
            && direct
            && confidence >= PROMOTION_CONFIDENCE
        {
            check
                .independent_sources
                .push((id, job_id, url, earlier_excerpt));
        }
    }
    Ok(check)
}

fn page_provenance(
    tx: &Transaction<'_>,
    job_id: &str,
    source_url: &str,
) -> io::Result<Option<serde_json::Value>> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT page_provenance_json FROM scholar_search_candidates
             WHERE job_id = ?1 AND fetched_url = ?2
             ORDER BY id DESC LIMIT 1",
            params![job_id, source_url],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .flatten();
    Ok(parse_page_provenance(raw.as_deref()))
}

fn parse_page_provenance(raw: Option<&str>) -> Option<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(raw?).ok()?;
    value.is_object().then_some(value)
}

fn provenance_link(
    current_url: &str,
    current: Option<&serde_json::Value>,
    earlier_url: &str,
    earlier: Option<&serde_json::Value>,
) -> Option<String> {
    let current_publications = publication_ids(current);
    let earlier_publications = publication_ids(earlier);
    if let Some(shared) = current_publications
        .intersection(&earlier_publications)
        .next()
    {
        return Some(format!(
            "both pages declare the same underlying publication identifier: {shared}"
        ));
    }
    let current_publishers = declared_publisher_ids(current);
    let earlier_publishers = declared_publisher_ids(earlier);
    if let Some(shared) = current_publishers.intersection(&earlier_publishers).next() {
        return Some(format!(
            "both pages declare the same publisher identifier: {shared}"
        ));
    }
    let current_canonical = current.and_then(|value| {
        value
            .get("canonical_url")
            .and_then(serde_json::Value::as_str)
    });
    let earlier_canonical = earlier.and_then(|value| {
        value
            .get("canonical_url")
            .and_then(serde_json::Value::as_str)
    });
    if current_canonical
        .zip(earlier_canonical)
        .is_some_and(|(left, right)| {
            normalized_provenance_url(left) == normalized_provenance_url(right)
        })
        || current_canonical.is_some_and(|canonical| {
            normalized_provenance_url(canonical) == normalized_provenance_url(earlier_url)
        })
        || earlier_canonical.is_some_and(|canonical| {
            normalized_provenance_url(canonical) == normalized_provenance_url(current_url)
        })
    {
        return Some("both pages declare the same canonical source".into());
    }
    let current_citations = declared_citations(current);
    let earlier_citations = declared_citations(earlier);
    let current_identity = current_canonical
        .map(normalized_provenance_url)
        .unwrap_or_else(|| normalized_provenance_url(current_url));
    let earlier_identity = earlier_canonical
        .map(normalized_provenance_url)
        .unwrap_or_else(|| normalized_provenance_url(earlier_url));
    if current_citations.contains(&earlier_identity)
        || earlier_citations.contains(&current_identity)
    {
        return Some("one page declares the other as a cited source".into());
    }
    if let Some(shared) = current_citations.intersection(&earlier_citations).next() {
        return Some(format!(
            "both pages declare the same cited source: {shared}"
        ));
    }
    None
}

fn publication_ids(value: Option<&serde_json::Value>) -> std::collections::BTreeSet<String> {
    value
        .and_then(|value| value.get("publication_ids"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(|identifier| identifier.trim().to_ascii_lowercase())
        .filter(|identifier| {
            (identifier.starts_with("doi:10.") || identifier.starts_with("pmid:"))
                && identifier.len() <= 300
        })
        .collect()
}

fn declared_publisher_ids(value: Option<&serde_json::Value>) -> std::collections::BTreeSet<String> {
    value
        .and_then(|value| value.get("publisher_ids"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter_map(|identifier| {
            let mut url = url::Url::parse(identifier).ok()?;
            if !matches!(url.scheme(), "https" | "http")
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return None;
            }
            url.set_query(None);
            url.set_fragment(None);
            Some(url.as_str().trim_end_matches('/').to_ascii_lowercase())
        })
        .collect()
}

fn declared_citations(value: Option<&serde_json::Value>) -> std::collections::BTreeSet<String> {
    value
        .and_then(|value| value.get("cited_urls"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .map(normalized_provenance_url)
        .filter(|url| !url.is_empty())
        .collect()
}

fn normalized_provenance_url(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value) else {
        return value.trim().trim_end_matches('/').to_ascii_lowercase();
    };
    url.set_fragment(None);
    url.set_query(None);
    url.as_str().trim_end_matches('/').to_ascii_lowercase()
}

// Compare ordered wording, not the fact itself. Recipes can independently say
// "chicken" or list the same ingredients; a long shared passage is different.
fn substantial_passage_overlap(left: &str, right: &str) -> bool {
    fn words(text: &str) -> Vec<String> {
        text.to_lowercase()
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .take(900)
            .map(str::to_owned)
            .collect()
    }
    let left = words(left);
    let right = words(right);
    if left.len() < 40 || right.len() < 40 {
        return false;
    }
    let (shorter, longer) = if left.len() <= right.len() {
        (&left, &right)
    } else {
        (&right, &left)
    };
    let minimum = 20.max(shorter.len() / 4);
    if shorter.len() < minimum {
        return false;
    }
    let long_windows = longer
        .windows(minimum)
        .collect::<std::collections::HashSet<_>>();
    shorter
        .windows(minimum)
        .any(|window| long_windows.contains(window))
}

pub(super) struct Assessment {
    pub topic_class: &'static str,
    pub source_kind: &'static str,
    pub topical_expertise: &'static str,
    pub direct_support: bool,
    pub independence_key: String,
    pub independence_verified: bool,
    pub currency: &'static str,
    pub published_at: Option<String>,
    pub gate_status: &'static str,
    pub reason: String,
    pub research_relevance: String,
}

#[cfg(test)]
pub(super) fn assess_scholar_finding(
    question: &str,
    source_url: &str,
    finding: &ScholarFinding,
    direct_support: bool,
    freshness: &'static str,
    profile: &ScholarSourceProfile,
    publication_is_recent: bool,
    has_independent_corroboration: bool,
) -> Assessment {
    assess_scholar_finding_with_relevance(
        question,
        source_url,
        finding,
        direct_support,
        freshness,
        profile,
        publication_is_recent,
        has_independent_corroboration,
        "unknown",
    )
}

pub(super) fn assess_scholar_finding_with_relevance(
    question: &str,
    source_url: &str,
    finding: &ScholarFinding,
    direct_support: bool,
    freshness: &'static str,
    profile: &ScholarSourceProfile,
    publication_is_recent: bool,
    has_independent_corroboration: bool,
    research_relevance: &str,
) -> Assessment {
    let research_relevance = match research_relevance {
        "direct" | "related" | "tangential" => research_relevance,
        _ => "unknown",
    };
    let wikipedia = source_url.starts_with("https://en.wikipedia.org/wiki/");
    let source_kind = if wikipedia {
        "secondary"
    } else {
        match finding.source_quality.as_str() {
            "primary" => "primary",
            "reference" => "secondary",
            _ => "unknown",
        }
    };
    let topical_expertise = match (source_kind, wikipedia) {
        ("primary", _) if profile.topical_expertise_verified => "verified_claim_specific_primary",
        ("primary", _) => "unverified_primary_expertise",
        ("secondary", true) => "general_encyclopedia",
        ("secondary", false) => "unassessed_secondary",
        _ => "unknown",
    };
    let independence_key = if wikipedia {
        "wikipedia.org".to_owned()
    } else if let Some(verified) = &profile.verified_underlying_source_id {
        canonical(verified)
    } else {
        source_url
            .split_once("://")
            .and_then(|(_, rest)| rest.split('/').next())
            .unwrap_or(source_url)
            .to_ascii_lowercase()
    };
    // A different hostname does not prove independent reporting. This only
    // becomes true after a source-assessment path verifies the underlying
    // publication; the current Wikipedia adapter cannot make that assertion.
    let independence_verified = !wikipedia && profile.verified_underlying_source_id.is_some();
    let topic_class = if is_consequential(question, &finding.subject, &finding.relation)
        || matches!(canonical(&finding.subject).as_str(), "user" | "the user")
        || [
            "lives in", "address", "email", "phone", "born on", "prefers",
        ]
        .iter()
        .any(|term| canonical(&finding.relation).contains(term))
    {
        "sensitive"
    } else if is_time_sensitive(question, &finding.relation, &finding.object) {
        "recent"
    } else {
        "historical_scientific"
    };
    let (gate_status, reason) = if !direct_support {
        (
            "unsupported",
            "claim is not directly supported by the exact source passage",
        )
    } else if topic_class == "sensitive" {
        ("pending_review", "sensitive claims require explicit review")
    } else if topic_class == "recent" && (profile.published_at.is_none() || !publication_is_recent)
    {
        (
            "unverified_temporal",
            "recent claim lacks a qualifying publication date",
        )
    } else if finding.confidence < PROMOTION_CONFIDENCE {
        (
            "pending_review",
            "extraction confidence is below the promotion threshold",
        )
    } else if source_kind == "unknown" {
        ("pending_review", "source quality is unknown")
    } else if topic_class == "recent" {
        if source_kind == "primary" && profile.topical_expertise_verified {
            (
                "eligible",
                "recent claim has direct, dated, expert primary evidence",
            )
        } else {
            (
                "pending_review",
                "recent claim needs an expert primary source",
            )
        }
    } else if source_kind == "primary" && profile.topical_expertise_verified {
        (
            "eligible",
            "directly supported by a claim-specific primary source",
        )
    } else if source_kind == "primary" {
        (
            "pending_review",
            "primary source expertise is not verified for this claim",
        )
    } else if source_kind == "secondary" && independence_verified && has_independent_corroboration {
        (
            "eligible",
            "two verified independent secondary sources directly support the claim",
        )
    } else {
        (
            "awaiting_corroboration",
            "one secondary source does not establish independent corroboration",
        )
    };
    Assessment {
        topic_class,
        source_kind,
        topical_expertise,
        direct_support,
        independence_key,
        independence_verified,
        currency: freshness,
        published_at: profile.published_at.clone(),
        gate_status,
        reason: reason.to_owned(),
        research_relevance: research_relevance.to_owned(),
    }
}

pub(super) fn record_assessment(
    tx: &Transaction<'_>,
    finding_id: i64,
    assessment: &Assessment,
    gate_status: &str,
    reason: &str,
) -> io::Result<()> {
    tx.execute(
        "INSERT INTO scholar_source_assessments
         (finding_id, topic_class, source_kind, topical_expertise, direct_support,
          independence_key, independence_verified, currency, published_at,
          gate_status, reason, research_relevance, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12,
                 strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
        params![
            finding_id,
            assessment.topic_class,
            assessment.source_kind,
            assessment.topical_expertise,
            i64::from(assessment.direct_support),
            assessment.independence_key,
            i64::from(assessment.independence_verified),
            assessment.currency,
            assessment.published_at,
            gate_status,
            reason,
            assessment.research_relevance,
        ],
    )
    .map_err(database_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declared_canonical_and_citation_links_flag_shared_origin() {
        let page_one: serde_json::Value = serde_json::from_str(
            r#"{"canonical_url":"https://one.example/story","cited_urls":["https://paper.example/report"]}"#,
        ).unwrap();
        let page_two_same_canonical: serde_json::Value = serde_json::from_str(
            r#"{"canonical_url":"https://one.example/story/","cited_urls":[]}"#,
        )
        .unwrap();
        assert_eq!(
            provenance_link(
                "https://mirror.example/story",
                Some(&page_two_same_canonical),
                "https://one.example/story",
                Some(&page_one),
            )
            .as_deref(),
            Some("both pages declare the same canonical source")
        );

        let page_two_shared_citation: serde_json::Value = serde_json::from_str(
            r#"{"canonical_url":"https://two.example/story","cited_urls":["https://paper.example/report"]}"#,
        ).unwrap();
        assert!(
            provenance_link(
                "https://two.example/story",
                Some(&page_two_shared_citation),
                "https://one.example/story",
                Some(&page_one),
            )
            .is_some_and(|reason| reason.contains("same cited source"))
        );
    }

    #[test]
    fn shared_common_fact_without_provenance_links_is_not_a_copy_signal() {
        let left = serde_json::json!({"canonical_url":"https://cook.example/a","cited_urls":[]});
        let right =
            serde_json::json!({"canonical_url":"https://kitchen.example/b","cited_urls":[]});
        assert!(
            provenance_link(
                "https://cook.example/a",
                Some(&left),
                "https://kitchen.example/b",
                Some(&right),
            )
            .is_none()
        );
    }

    #[test]
    fn matching_stable_publication_ids_flag_shared_reporting() {
        let left = serde_json::json!({"publication_ids":["doi:10.1234/report-a"]});
        let same_report = serde_json::json!({"publication_ids":["doi:10.1234/REPORT-A"]});
        let distinct_report = serde_json::json!({"publication_ids":["doi:10.1234/report-b"]});
        assert_eq!(
            provenance_link(
                "https://one.example/a",
                Some(&left),
                "https://two.example/a",
                Some(&same_report),
            ),
            Some("both pages declare the same underlying publication identifier: doi:10.1234/report-a".into())
        );
        assert_eq!(
            provenance_link(
                "https://one.example/a",
                Some(&left),
                "https://two.example/b",
                Some(&distinct_report),
            ),
            None
        );
    }

    #[test]
    fn matching_structured_publisher_ids_flag_shared_ownership_for_review() {
        let left = serde_json::json!({"publisher_ids":["https://group.example/publisher"]});
        let same_publisher =
            serde_json::json!({"publisher_ids":["https://GROUP.example/publisher/"]});
        let separate_publisher =
            serde_json::json!({"publisher_ids":["https://other.example/publisher"]});
        assert_eq!(
            provenance_link(
                "https://one.example/a",
                Some(&left),
                "https://two.example/a",
                Some(&same_publisher),
            ),
            Some(
                "both pages declare the same publisher identifier: https://group.example/publisher"
                    .into()
            )
        );
        assert_eq!(
            provenance_link(
                "https://one.example/a",
                Some(&left),
                "https://two.example/b",
                Some(&separate_publisher),
            ),
            None
        );
    }

    #[test]
    fn wikipedia_source_is_staged_by_topic_class() {
        let finding = ScholarFinding {
            semantic_support: None,
            subject: "Photosynthesis".into(),
            relation: "converts".into(),
            object: "light energy".into(),
            evidence_text: "Photosynthesis converts light energy.".into(),
            confidence: 0.99,
            valid_from: None,
            valid_until: None,
            source_quality: "reference".into(),
            source_updated_at: None,
            proposition: Default::default(),
        };
        let url = "https://en.wikipedia.org/wiki/Photosynthesis";
        let stable = assess_scholar_finding(
            "How does photosynthesis work?",
            url,
            &finding,
            true,
            "not_time_sensitive",
            &ScholarSourceProfile::default(),
            false,
            false,
        );
        assert_eq!(stable.topic_class, "historical_scientific");
        assert_eq!(stable.source_kind, "secondary");
        assert!(!stable.independence_verified);
        assert_eq!(stable.gate_status, "awaiting_corroboration");

        let recent = assess_scholar_finding(
            "What is the current version?",
            url,
            &finding,
            true,
            "fresh",
            &ScholarSourceProfile::default(),
            false,
            false,
        );
        assert_eq!(recent.topic_class, "recent");
        assert_eq!(recent.gate_status, "unverified_temporal");
        let gpu = assess_scholar_finding(
            "What is the GPU specification?",
            url,
            &finding,
            true,
            "fresh",
            &ScholarSourceProfile::default(),
            false,
            false,
        );
        assert_eq!(gpu.topic_class, "recent");

        let sensitive = assess_scholar_finding(
            "Is this medical treatment safe?",
            url,
            &finding,
            true,
            "not_time_sensitive",
            &ScholarSourceProfile::default(),
            false,
            false,
        );
        assert_eq!(sensitive.topic_class, "sensitive");
        assert_eq!(sensitive.gate_status, "pending_review");

        let mut primary_finding = finding.clone();
        primary_finding.source_quality = "primary".into();
        let primary = assess_scholar_finding(
            "How does photosynthesis work?",
            "https://research.example.org/experiment",
            &primary_finding,
            true,
            "not_time_sensitive",
            &ScholarSourceProfile {
                topical_expertise_verified: true,
                ..Default::default()
            },
            false,
            false,
        );
        assert_eq!(primary.source_kind, "primary");
        assert_eq!(primary.gate_status, "eligible");
        let unverified_primary = assess_scholar_finding(
            "How does photosynthesis work?",
            "https://research.example.org/experiment",
            &primary_finding,
            true,
            "not_time_sensitive",
            &ScholarSourceProfile::default(),
            false,
            false,
        );
        assert_eq!(unverified_primary.gate_status, "pending_review");
    }

    #[test]
    fn chicken_stock_does_not_make_an_ingredient_claim_time_sensitive() {
        let finding = ScholarFinding {
            semantic_support: None,
            subject: "Simple Chicken Soup".into(),
            relation: "lists ingredients".into(),
            object: "Chicken breasts; Chicken stock; Carrots".into(),
            evidence_text:
                "## Simple Chicken Soup Ingredients\n* Chicken breasts\n* Chicken stock\n* Carrots"
                    .into(),
            confidence: 0.8,
            valid_from: None,
            valid_until: None,
            source_quality: "unknown".into(),
            source_updated_at: None,
            proposition: Default::default(),
        };
        let assessment = assess_scholar_finding(
            "What are common ingredients in chicken soup?",
            "https://example.org/chicken-soup",
            &finding,
            true,
            "not_time_sensitive",
            &ScholarSourceProfile::default(),
            false,
            false,
        );
        assert_eq!(assessment.topic_class, "historical_scientific");
        assert_eq!(assessment.gate_status, "pending_review");
    }
}
