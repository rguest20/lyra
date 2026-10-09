use super::*;

fn provenance_allows_pair(tx: &Transaction<'_>, first_id: i64, second_id: i64) -> io::Result<bool> {
    let unresolved: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM scholar_provenance_reviews
             WHERE ((finding_id = ?1 AND related_finding_id = ?2)
                OR (finding_id = ?2 AND related_finding_id = ?1))
               AND (status <> 'reviewed'
                    OR COALESCE(verdict, '') <> 'possibly_distinct_reporting'))",
            params![first_id, second_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    Ok(!unresolved)
}

fn reviewed_origin(tx: &Transaction<'_>, finding_id: i64) -> io::Result<Option<String>> {
    tx.query_row(
        "SELECT underlying_source_id FROM scholar_source_origin_reviews
         WHERE finding_id = ?1 AND review_method = 'human'
         ORDER BY id DESC LIMIT 1",
        [finding_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(database_error)
}

pub(super) fn self_promote_verified_findings(
    tx: &Transaction<'_>,
    subject: &str,
    relation: &str,
    object: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
) -> io::Result<()> {
    let findings: Vec<(i64, String, String, String, String, String)> = {
        let mut statement = tx
            .prepare(
                "SELECT f.id, f.job_id, f.source_url, f.source_excerpt, f.status,
                    a.independence_key
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE (lower(trim(f.subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(f.subject)) AND e.right_name = lower(trim(?1))))
                   AND lower(trim(f.relation)) = lower(trim(?2))
                   AND lower(trim(f.object)) = lower(trim(?3))
                   AND f.valid_from IS ?4 AND f.valid_until IS ?5
                   AND f.status IN ('needs_review', 'promoted')
                   AND f.confidence >= ?6
                   AND a.source_kind = 'secondary'
                   AND a.direct_support = 1 AND a.independence_verified = 1
                   AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews er
                        WHERE er.finding_id = f.id ORDER BY er.id DESC LIMIT 1) = 1
                   AND (SELECT underlying_source_id FROM scholar_source_origin_reviews origin
                        WHERE origin.finding_id = f.id ORDER BY origin.id DESC LIMIT 1) = a.independence_key
                 ORDER BY f.id",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    subject,
                    relation,
                    object,
                    valid_from,
                    valid_until,
                    automatic_promotion_confidence(),
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };

    let mut independent_pair = None;
    'pairs: for (index, first) in findings.iter().enumerate() {
        for second in findings.iter().skip(index + 1) {
            if first.5 == second.5 {
                continue;
            }
            if provenance_allows_pair(tx, first.0, second.0)? {
                independent_pair = Some((first, second));
                break 'pairs;
            }
        }
    }
    let Some((first, second)) = independent_pair else {
        return Ok(());
    };

    let today: String = tx
        .query_row("SELECT date('now')", [], |row| row.get(0))
        .map_err(database_error)?;
    let conflicting_id: Option<i64> = {
        let mut statement = tx
            .prepare(
                "SELECT id, object, valid_from, valid_until FROM semantic_claims
                 WHERE retracted_by IS NULL AND (lower(trim(subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(subject)) AND e.right_name = lower(trim(?1))))
                   AND lower(trim(relation)) = lower(trim(?2))",
            )
            .map_err(database_error)?;
        let claims = statement
            .query_map(params![subject, relation], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        claims
            .into_iter()
            .find_map(|(id, current_object, from, until)| {
                (canonical(&current_object) != canonical(object)
                    && claim_periods_overlap(
                        valid_from,
                        valid_until,
                        from.as_deref(),
                        until.as_deref(),
                        &today,
                    ))
                .then_some(id)
            })
    };
    if let Some(id) = conflicting_id {
        tx.execute(
            "UPDATE scholar_source_assessments SET gate_status = 'conflict',
                reason = ?3
             WHERE finding_id IN (?1, ?2)",
            params![
                first.0,
                second.0,
                format!("automatic acceptance stopped by active conflicting claim #{id}")
            ],
        )
        .map_err(database_error)?;
        return Ok(());
    }

    let claim_id: Option<i64> = tx
        .query_row(
            "SELECT id FROM semantic_claims WHERE retracted_by IS NULL
               AND (lower(trim(subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(subject)) AND e.right_name = lower(trim(?1))))
               AND lower(trim(relation)) = lower(trim(?2))
               AND lower(trim(object)) = lower(trim(?3))
               AND valid_from IS ?4 AND valid_until IS ?5
             ORDER BY id DESC LIMIT 1",
            params![subject, relation, object, valid_from, valid_until],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let claim_id = if let Some(id) = claim_id {
        id
    } else {
        tx.execute(
            "INSERT INTO semantic_claims
                (subject, relation, object, source, source_observation_id, created_at, valid_from, valid_until)
             VALUES (?1, ?2, ?3, ?4, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?5, ?6)",
            params![subject, relation, object, first.2, valid_from, valid_until],
        )
        .map_err(database_error)?;
        tx.last_insert_rowid()
    };

    for finding in [first, second] {
        tx.execute(
            "UPDATE scholar_findings SET status = 'promoted', semantic_claim_id = ?2,
                decision_reason = 'automatically accepted after two directly supporting, high-confidence findings with suitable topics and distinct verified publication origins'
             WHERE id = ?1 AND status = 'needs_review'",
            params![finding.0, claim_id],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_source_assessments SET reason = substr(reason, 1, 900) ||
                '; automatically accepted with independent high-confidence corroboration'
             WHERE finding_id = ?1",
            [finding.0],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_jobs SET status = 'promoted', last_error = NULL
             WHERE id = ?1 AND NOT EXISTS (
                SELECT 1 FROM scholar_findings pending
                WHERE pending.job_id = ?1 AND pending.status = 'needs_review'
             ) AND status <> 'processing'",
            [&finding.1],
        )
        .map_err(database_error)?;
        link_claim_evidence(
            tx,
            claim_id,
            "web",
            Some(&finding.2),
            None,
            &finding.3,
            "supports",
        )?;

        if finding.4 == "needs_review" {
            let proposal_id: i64 = tx
                .query_row(
                    "SELECT id FROM role_proposals WHERE source_type = 'scholar_finding'
                       AND source_id = ?1 ORDER BY id DESC LIMIT 1",
                    [finding.0.to_string()],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            record_policy_decision(
                tx,
                proposal_id,
                "automatic_evidence_acceptance",
                &format!(
                    "Accepted at {:.2}+ extraction confidence after exact-passage support, claim-specific suitability assessment, and two distinct independent-source assessments; no provenance warning or active map conflict was found.",
                    automatic_promotion_confidence()
                ),
                Some(claim_id),
                Some("active"),
            )?;
        }
    }
    Ok(())
}

fn automatic_source_host(source_url: &str) -> Option<String> {
    let url = url::Url::parse(source_url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches("www.")
        .to_ascii_lowercase();
    (!host.is_empty()).then(|| format!("host:{host}"))
}

struct SourceCandidate {
    id: i64,
    url: String,
    evidence_text: String,
}

struct ClaimKey {
    subject: String,
    relation: String,
    object: String,
    valid_from: Option<String>,
    valid_until: Option<String>,
}

pub(super) fn automatically_review_source_pair(
    tx: &Transaction<'_>,
    finding_id: i64,
) -> io::Result<()> {
    let current: Option<ClaimKey> = tx
        .query_row(
            "SELECT subject, relation, object, valid_from, valid_until
             FROM scholar_findings WHERE id = ?1",
            [finding_id],
            |row| {
                Ok(ClaimKey {
                    subject: row.get(0)?,
                    relation: row.get(1)?,
                    object: row.get(2)?,
                    valid_from: row.get(3)?,
                    valid_until: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?;
    let Some(ClaimKey {
        subject,
        relation,
        object,
        valid_from,
        valid_until,
    }) = current
    else {
        return Ok(());
    };
    if matches!(canonical(&subject).as_str(), "user" | "the user") {
        return Ok(());
    }

    let rows: Vec<SourceCandidate> = {
        let mut statement = tx
            .prepare(
                "SELECT f.id, f.source_url, f.evidence_text
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE (lower(trim(f.subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(f.subject)) AND e.right_name = lower(trim(?1))))
                   AND lower(trim(f.relation)) = lower(trim(?2))
                   AND lower(trim(f.object)) = lower(trim(?3))
                   AND f.valid_from IS ?4 AND f.valid_until IS ?5
                   AND f.status = 'needs_review'
                   AND f.confidence >= ?6
                   AND a.direct_support = 1
                   AND a.research_relevance = 'direct'
                   AND instr(f.source_excerpt, f.evidence_text) > 0
                   AND lower(trim(f.subject)) NOT IN ('user', 'the user')
                   AND f.source_quality <> 'primary'
                 ORDER BY f.id DESC LIMIT 40",
            )
            .map_err(database_error)?;
        statement
            .query_map(
                params![
                    subject,
                    relation,
                    object,
                    valid_from,
                    valid_until,
                    automatic_promotion_confidence(),
                ],
                |row| {
                    Ok(SourceCandidate {
                        id: row.get(0)?,
                        url: row.get(1)?,
                        evidence_text: row.get(2)?,
                    })
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };
    let mut selected_pair = None;
    'pairs: for (index, left) in rows.iter().enumerate() {
        let Some(left_host) = automatic_source_host(&left.url) else {
            continue;
        };
        for right in rows.iter().skip(index + 1) {
            let Some(right_host) = automatic_source_host(&right.url) else {
                continue;
            };
            if left_host == right_host {
                continue;
            }
            if !provenance_allows_pair(tx, left.id, right.id)? {
                continue;
            }
            let has_unsuitable_review: bool = tx
                .query_row(
                    "SELECT COALESCE((SELECT suitable_for_topic
                     FROM scholar_source_expertise_reviews
                     WHERE finding_id = ?1 ORDER BY id DESC LIMIT 1), 1) = 0
                     OR COALESCE((SELECT suitable_for_topic
                     FROM scholar_source_expertise_reviews
                     WHERE finding_id = ?2 ORDER BY id DESC LIMIT 1), 1) = 0",
                    params![left.id, right.id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if has_unsuitable_review {
                continue;
            }
            let left_reviewed_origin = reviewed_origin(tx, left.id)?;
            let right_reviewed_origin = reviewed_origin(tx, right.id)?;
            let left_origin = left_reviewed_origin.unwrap_or_else(|| left_host.clone());
            let right_origin = right_reviewed_origin.unwrap_or_else(|| right_host.clone());
            if left_origin == right_origin {
                continue;
            }
            selected_pair = Some((left, right, left_origin, right_origin));
            break 'pairs;
        }
    }
    let Some((left, right, left_origin, right_origin)) = selected_pair else {
        return Ok(());
    };

    for (finding, origin) in [(left, left_origin), (right, right_origin)] {
        tx.execute(
            "INSERT INTO scholar_source_expertise_reviews
                (finding_id, source_kind, suitable_for_topic, published_at,
                 evidence_text, rationale, reviewed_at, review_method)
             SELECT ?1, 'secondary', 1, NULL, ?2,
                'Automatic assessment: exact passage directly supports the claim and research relevance is direct.',
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'automatic'
             WHERE NOT EXISTS (SELECT 1 FROM scholar_source_expertise_reviews
                               WHERE finding_id = ?1 AND review_method = 'automatic')",
            params![finding.id, finding.evidence_text],
        )
        .map_err(database_error)?;
        tx.execute(
            "INSERT INTO scholar_source_origin_reviews
                (finding_id, underlying_source_id, evidence_text, rationale, reviewed_at, review_method)
             SELECT ?1, ?2, ?3,
                'Automatic assessment: distinct source origin; no unresolved citation, canonical, or copied-passage signal.',
                strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), 'automatic'
             WHERE NOT EXISTS (SELECT 1 FROM scholar_source_origin_reviews
                               WHERE finding_id = ?1 AND review_method = 'automatic')
               AND NOT EXISTS (SELECT 1 FROM scholar_source_origin_reviews
                               WHERE finding_id = ?1 AND review_method = 'human')",
            params![finding.id, origin, finding.evidence_text],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_findings SET source_quality = 'reference' WHERE id = ?1",
            [finding.id],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_source_assessments SET source_kind = 'secondary',
                topical_expertise = 'automatic_direct_topic_match',
                independence_key = ?2, independence_verified = 1,
                gate_status = 'eligible',
                reason = 'automatic review: direct passage and topic match; distinct source host; confidence meets configured threshold'
             WHERE finding_id = ?1",
            params![finding.id, origin],
        )
        .map_err(database_error)?;
    }
    self_promote_verified_findings(
        tx,
        &subject,
        &relation,
        &object,
        valid_from.as_deref(),
        valid_until.as_deref(),
    )
}

impl LearningStore {
    pub fn automatically_review_pending_scholar_evidence(&mut self) -> io::Result<usize> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let ids: Vec<i64> = {
            let mut statement = tx
                .prepare(
                    "SELECT f.id FROM scholar_findings f
                     JOIN scholar_source_assessments a ON a.finding_id = f.id
                     WHERE f.status = 'needs_review' AND a.direct_support = 1
                       AND f.confidence >= ?1
                     ORDER BY f.id LIMIT 100",
                )
                .map_err(database_error)?;
            statement
                .query_map([automatic_promotion_confidence()], |row| row.get(0))
                .map_err(database_error)?
                .collect::<Result<Vec<i64>, _>>()
                .map_err(database_error)?
        };
        let mut promoted = 0;
        for id in ids {
            automatically_review_source_pair(&tx, id)?;
            let status: String = tx
                .query_row(
                    "SELECT status FROM scholar_findings WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .map_err(database_error)?;
            if status == "promoted" {
                promoted = 1;
                break;
            }
        }
        tx.commit().map_err(database_error)?;
        Ok(promoted)
    }
}
