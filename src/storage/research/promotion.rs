use super::*;

struct PromotionFinding {
    id: i64,
    job_id: String,
    source_url: String,
    source_excerpt: String,
    status: String,
    independence_key: String,
    proposition: PropositionMetadata,
}

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
    proposition: &ScholarPropositionScope,
) -> io::Result<()> {
    let findings: Vec<PromotionFinding> = {
        let query = format!(
                "WITH {AUTOMATIC_PROMOTION_ELIGIBILITY_CTE}
                 SELECT f.id, f.job_id, f.source_url, f.source_excerpt, f.status,
                    f.independence_key, f.attribution, f.polarity, f.modality,
                    f.geographic_scope, f.context_scope
                 FROM eligible_findings f
                 WHERE (lower(trim(f.subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(f.subject)) AND e.right_name = lower(trim(?1))))
                   AND lower(trim(f.relation)) = lower(trim(?2))
                   AND lower(trim(f.object)) = lower(trim(?3))
                   AND f.valid_from IS ?4 AND f.valid_until IS ?5
                   AND f.geographic_scope IS ?6 AND f.context_scope IS ?7
                   AND (SELECT underlying_source_id FROM scholar_source_origin_reviews origin
                        WHERE origin.finding_id = f.id AND origin.review_method = 'human'
                        ORDER BY origin.id DESC LIMIT 1) = f.independence_key
                 ORDER BY f.id",
            );
        let mut statement = tx.prepare(&query).map_err(database_error)?;
        statement
            .query_map(
                params![
                    proposition.subject,
                    proposition.relation,
                    proposition.proposition.object,
                    proposition.proposition.valid_from,
                    proposition.proposition.valid_until,
                    proposition.proposition.geographic_scope,
                    proposition.proposition.context_scope,
                    automatic_promotion_confidence(),
                ],
                |row| {
                    Ok(PromotionFinding {
                        id: row.get(0)?,
                        job_id: row.get(1)?,
                        source_url: row.get(2)?,
                        source_excerpt: row.get(3)?,
                        status: row.get(4)?,
                        independence_key: row.get(5)?,
                        proposition: PropositionMetadata::from_storage_row(row, 6)?,
                    })
                },
            )
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?
    };

    let mut independent_pair = None;
    'pairs: for (index, first) in findings.iter().enumerate() {
        for second in findings.iter().skip(index + 1) {
            if first.independence_key == second.independence_key
                || first.proposition != second.proposition
            {
                continue;
            }
            if provenance_allows_pair(tx, first.id, second.id)? {
                independent_pair = Some((first, second));
                break 'pairs;
            }
        }
    }
    let Some((first, second)) = independent_pair else {
        return Ok(());
    };

    let today = crate::storage::current_sql_date(tx)?;
    let conflicting_id: Option<i64> = {
        let mut statement = tx
            .prepare(
                "SELECT id, object, valid_from, valid_until, geographic_scope, context_scope FROM semantic_claims
                 WHERE retracted_by IS NULL AND (lower(trim(subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(subject)) AND e.right_name = lower(trim(?1))))
                   AND lower(trim(relation)) = lower(trim(?2))",
            )
            .map_err(database_error)?;
        let claims = statement
            .query_map(params![proposition.subject, proposition.relation], |row| {
                super::SemanticClaimScope::from_row(row, 0)
            })
            .map_err(database_error)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(database_error)?;
        claims.into_iter().find_map(|claim| {
            (claim.proposition.same_scope(
                &proposition.proposition.geographic_scope,
                &proposition.proposition.context_scope,
            ) && canonical(&claim.proposition.object)
                != canonical(&proposition.proposition.object)
                && claim_periods_overlap(
                    proposition.proposition.valid_from.as_deref(),
                    proposition.proposition.valid_until.as_deref(),
                    claim.proposition.valid_from.as_deref(),
                    claim.proposition.valid_until.as_deref(),
                    &today,
                ))
            .then_some(claim.id)
        })
    };
    if let Some(id) = conflicting_id {
        for finding in [first, second] {
            super::link_scholar_finding_claim(
                tx,
                finding.id,
                id,
                "contradicts",
                "same subject and relation with a different object and overlapping validity period; policy withheld promotion",
                "automatic_promotion_conflict_check",
            )?;
        }
        tx.execute(
            "UPDATE scholar_source_assessments SET gate_status = 'conflict',
                reason = ?3
             WHERE finding_id IN (?1, ?2)",
            params![
                first.id,
                second.id,
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
            params![proposition.subject, proposition.relation, proposition.proposition.object,
                proposition.proposition.valid_from, proposition.proposition.valid_until,
                proposition.proposition.geographic_scope, proposition.proposition.context_scope],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?;
    let claim_id = if let Some(id) = claim_id {
        id
    } else {
        super::insert_scholar_semantic_claim(
            tx,
            super::NewSemanticClaim {
                subject: &proposition.subject,
                relation: &proposition.relation,
                object: &proposition.proposition.object,
                source: &first.source_url,
                supersedes_id: None,
                valid_from: proposition.proposition.valid_from.as_deref(),
                valid_until: proposition.proposition.valid_until.as_deref(),
                proposition: &first.proposition,
            },
        )?
    };

    for finding in [first, second] {
        super::promote_scholar_finding(
            tx,
            finding.id,
            claim_id,
            "automatically accepted after two directly supporting, high-confidence findings with suitable topics and distinct verified publication origins",
        )?;
        super::link_scholar_finding_claim(
            tx,
            finding.id,
            claim_id,
            "supports",
            "finding has the same proposition and validity period as the claim; the pair passed independent-source promotion policy",
            "two_source_policy_promotion",
        )?;
        tx.execute(
            "UPDATE scholar_source_assessments SET reason = substr(reason, 1, 900) ||
                '; automatically accepted with independent high-confidence corroboration'
             WHERE finding_id = ?1",
            [finding.id],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_jobs SET status = 'promoted', last_error = NULL
             WHERE id = ?1 AND NOT EXISTS (
                SELECT 1 FROM scholar_findings pending
                WHERE pending.job_id = ?1 AND pending.status = 'needs_review'
             ) AND status <> 'processing'",
            [&finding.job_id],
        )
        .map_err(database_error)?;
        tx.execute(
            "UPDATE scholar_task_targets SET stop_reason='complete',updated_at=strftime('%Y-%m-%dT%H:%M:%fZ','now')
             WHERE job_id=?1 AND (SELECT status FROM scholar_jobs WHERE id=?1)='promoted'",
            [&finding.job_id],
        )
        .map_err(database_error)?;
        link_claim_evidence(
            tx,
            claim_id,
            "web",
            Some(&finding.source_url),
            None,
            &finding.source_excerpt,
            "supports",
        )?;

        if finding.status == "needs_review" {
            let proposal_id = super::latest_scholar_finding_proposal_id(tx, finding.id)?;
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
