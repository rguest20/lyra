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

pub(super) fn self_promote_verified_findings(
    tx: &Transaction<'_>,
    subject: &str,
    relation: &str,
    object: &str,
    valid_from: Option<&str>,
    valid_until: Option<&str>,
    geographic_scope: Option<&str>,
    context_scope: Option<&str>,
) -> io::Result<()> {
    let findings: Vec<(
        i64,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        String,
        String,
        Option<String>,
        Option<String>,
    )> = {
        let mut statement = tx
            .prepare(
                "SELECT f.id, f.job_id, f.source_url, f.source_excerpt, f.status,
                    a.independence_key, f.attribution, f.polarity, f.modality,
                    f.geographic_scope, f.context_scope
                 FROM scholar_findings f
                 JOIN scholar_source_assessments a ON a.finding_id = f.id
                 WHERE (lower(trim(f.subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(f.subject)) AND e.right_name = lower(trim(?1))))
                   AND lower(trim(f.relation)) = lower(trim(?2))
                   AND lower(trim(f.object)) = lower(trim(?3))
                   AND f.valid_from IS ?4 AND f.valid_until IS ?5
                   AND f.geographic_scope IS ?6 AND f.context_scope IS ?7
                   AND f.status IN ('needs_review', 'promoted')
                   AND f.confidence >= ?8
                   AND a.source_kind = 'secondary'
                   AND a.direct_support = 1 AND a.independence_verified = 1
                   AND (SELECT suitable_for_topic FROM scholar_source_expertise_reviews er
                        WHERE er.finding_id = f.id ORDER BY er.id DESC LIMIT 1) = 1
                   AND (SELECT underlying_source_id FROM scholar_source_origin_reviews origin
                        WHERE origin.finding_id = f.id AND origin.review_method = 'human'
                        ORDER BY origin.id DESC LIMIT 1) = a.independence_key
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
                    geographic_scope,
                    context_scope,
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
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
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
            if first.5 == second.5
                || first.6 != second.6
                || first.7 != second.7
                || first.8 != second.8
                || first.9 != second.9
                || first.10 != second.10
            {
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
                "SELECT id, object, valid_from, valid_until, geographic_scope, context_scope FROM semantic_claims
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
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        claims
            .into_iter()
            .find_map(|(id, current_object, from, until, geo, context)| {
                (geo.as_deref() == geographic_scope
                    && context.as_deref() == context_scope
                    && canonical(&current_object) != canonical(object)
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
        for finding in [first, second] {
            tx.execute(
                "INSERT OR IGNORE INTO scholar_finding_claim_links
                    (finding_id,claim_id,relation,rationale,assessment_method)
                 VALUES (?1,?2,'contradicts',?3,'automatic_promotion_conflict_check')",
                params![
                    finding.0,
                    id,
                    "same subject and relation with a different object and overlapping validity period; policy withheld promotion"
                ],
            )
            .map_err(database_error)?;
        }
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
               AND geographic_scope IS ?6 AND context_scope IS ?7
             ORDER BY id DESC LIMIT 1",
            params![subject, relation, object, valid_from, valid_until, geographic_scope, context_scope],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let claim_id = if let Some(id) = claim_id {
        id
    } else {
        tx.execute(
            "INSERT INTO semantic_claims
                (subject, relation, object, source, source_observation_id, created_at, valid_from, valid_until, attribution, polarity, modality, geographic_scope, context_scope)
             VALUES (?1, ?2, ?3, ?4, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![subject, relation, object, first.2, valid_from, valid_until, first.6, first.7, first.8, geographic_scope, context_scope],
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
            "INSERT OR IGNORE INTO scholar_finding_claim_links
                (finding_id,claim_id,relation,rationale,assessment_method)
             VALUES (?1,?2,'supports',?3,'two_source_policy_promotion')",
            params![
                finding.0,
                claim_id,
                "finding has the same proposition and validity period as the claim; the pair passed independent-source promotion policy"
            ],
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
        tx.execute(
            "UPDATE scholar_task_targets SET stop_reason='complete',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE job_id=?1 AND (SELECT status FROM scholar_jobs WHERE id=?1)='promoted'",
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

impl LearningStore {
    /// Host diversity is a discovery signal, not verification of independent
    /// reporting. Clear stale map-conflict gates after the conflict is retracted,
    /// but keep the finding provisional until explicit origin reviews unlock it.
    pub fn automatically_review_pending_scholar_evidence(&mut self) -> io::Result<usize> {
        self.connection.execute(
            "UPDATE scholar_source_assessments SET gate_status='awaiting_corroboration',
                reason='map conflict was retracted; distinct hosts alone do not prove independent reporting'
             WHERE gate_status='conflict' AND reason LIKE '%overlapping semantic claim #%'
               AND EXISTS (SELECT 1 FROM scholar_findings f
                           WHERE f.id=scholar_source_assessments.finding_id
                             AND f.status='needs_review')
               AND NOT EXISTS (
                 SELECT 1 FROM scholar_findings f
                 JOIN semantic_claims c ON c.retracted_by IS NULL
                   AND c.status IN ('active','uncertain','challenged')
                   AND (lower(trim(c.subject))=lower(trim(f.subject)) OR EXISTS (
                       SELECT 1 FROM entity_equivalences e
                       WHERE e.left_name=lower(trim(c.subject))
                         AND e.right_name=lower(trim(f.subject))))
                   AND lower(trim(c.relation))=lower(trim(f.relation))
                   AND lower(trim(c.object))<>lower(trim(f.object))
                   AND c.geographic_scope IS f.geographic_scope
                   AND c.context_scope IS f.context_scope
                   AND (f.valid_from IS NULL OR c.valid_until IS NULL OR f.valid_from<=c.valid_until)
                   AND (f.valid_until IS NULL OR c.valid_from IS NULL OR c.valid_from<=f.valid_until)
                 WHERE f.id=scholar_source_assessments.finding_id
               )",
            [],
        ).map_err(database_error)?;
        Ok(0)
    }
}
