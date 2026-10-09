use super::*;

fn scholar_source_freshness(
    tx: &Transaction<'_>,
    finding: &ScholarFinding,
    time_sensitive: bool,
) -> io::Result<&'static str> {
    if !time_sensitive {
        return Ok("not_time_sensitive");
    }
    let Some(updated_at) = finding.source_updated_at.as_deref() else {
        return Ok("unknown");
    };
    let age_days: Option<f64> = tx
        .query_row(
            "SELECT julianday('now') - julianday(?1)",
            [updated_at],
            |row| row.get(0),
        )
        .optional()
        .map_err(database_error)?
        .flatten();
    Ok(match age_days {
        Some(days) if (0.0..=365.0).contains(&days) => "fresh",
        Some(days) if days > 365.0 => "stale",
        _ => "unknown",
    })
}

fn grounded_scholar_finding(
    source_title: &str,
    source_excerpt: &str,
    finding: &ScholarFinding,
) -> bool {
    let safe_field = |value: &str| {
        !value.trim().is_empty()
            && value.chars().count() <= 500
            && !value.chars().any(char::is_control)
            && !value.contains("<|")
    };
    let safe_evidence = !finding.evidence_text.trim().is_empty()
        && finding.evidence_text.chars().count() <= 4_500
        && !finding
            .evidence_text
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
        && !finding.evidence_text.contains("<|");
    let semantic_valid = finding
        .semantic_support
        .as_ref()
        .is_none_or(|support| support.supports(finding, source_excerpt));
    semantic_valid
        && safe_evidence
        && source_excerpt.contains(&finding.evidence_text)
        && safe_field(&finding.subject)
        && safe_field(&finding.relation)
        && safe_field(&finding.object)
        && (finding
            .semantic_support
            .as_ref()
            .is_some_and(|support| support.supports(finding, source_excerpt))
            || evidence_supports_claim(finding)
            || (matches!(
                finding.relation.as_str(),
                "has encyclopedia summary" | "has encyclopedia detail"
            ) && canonical(&finding.subject) == canonical(source_title)
                && finding.object == finding.evidence_text))
        && finding.confidence.is_finite()
        && (0.0..=1.0).contains(&finding.confidence)
        && valid_claim_period(
            finding.valid_from.as_deref(),
            finding.valid_until.as_deref(),
        )
        && [
            finding.valid_from.as_deref(),
            finding.valid_until.as_deref(),
        ]
        .into_iter()
        .flatten()
        .all(|date| source_excerpt.contains(date))
}

struct IngestedFinding {
    id: i64,
    status: &'static str,
    reason: Option<String>,
}

fn existing_semantic_match(
    tx: &Transaction<'_>,
    finding: &ScholarFinding,
    today: &str,
) -> io::Result<(Option<i64>, Option<i64>)> {
    let mut statement = tx
        .prepare(
            "SELECT id, object, valid_from, valid_until FROM semantic_claims
             WHERE retracted_by IS NULL
               AND (lower(trim(subject)) = lower(trim(?1)) OR EXISTS (SELECT 1 FROM entity_equivalences e WHERE e.left_name = lower(trim(subject)) AND e.right_name = lower(trim(?1))))
               AND lower(trim(relation)) = lower(trim(?2))
             ORDER BY id DESC",
        )
        .map_err(database_error)?;
    let existing = statement
        .query_map(params![finding.subject, finding.relation], |row| {
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
    for (id, object, valid_from, valid_until) in existing {
        if !claim_periods_overlap(
            finding.valid_from.as_deref(),
            finding.valid_until.as_deref(),
            valid_from.as_deref(),
            valid_until.as_deref(),
            today,
        ) {
            continue;
        }
        if canonical(&object) == canonical(&finding.object)
            && valid_from.as_deref() == finding.valid_from.as_deref()
            && valid_until.as_deref() == finding.valid_until.as_deref()
        {
            return Ok((Some(id), None));
        }
        if canonical(&object) != canonical(&finding.object)
            && finding.relation != "has encyclopedia detail"
        {
            return Ok((None, Some(id)));
        }
    }
    Ok((None, None))
}

fn record_provenance_review(
    tx: &Transaction<'_>,
    finding_id: i64,
    corroboration: &super::evidence_policy::CorroborationCheck,
) -> io::Result<()> {
    let provenance_review = corroboration
        .provenance_link
        .as_ref()
        .cloned()
        .or_else(|| {
            corroboration.possible_copy_of.map(|related_id| {
                (
                    related_id,
                    "passage substantially overlaps the earlier source; possible copied reporting"
                        .into(),
                )
            })
        })
        .or_else(|| {
            corroboration.shared_source_with.map(|related_id| {
                (
                    related_id,
                    "both findings identify the same verified underlying source".into(),
                )
            })
        });
    if let Some((related_finding_id, provenance_signal)) = provenance_review {
        tx.execute(
            "INSERT OR IGNORE INTO scholar_provenance_reviews
                     (finding_id, related_finding_id, signal, status, created_at)
                     VALUES (?1, ?2, ?3, 'pending', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))",
            params![finding_id, related_finding_id, provenance_signal],
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn ingest_source(
    tx: &Transaction<'_>,
    job_id: &str,
    source: &ScholarEvidence,
) -> io::Result<Option<IngestedFinding>> {
    let ScholarEvidence {
        source_title,
        source_url,
        source_excerpt,
        finding,
        profile,
        research_relevance,
    } = source;
    let Some(finding) = finding.as_ref() else {
        return Ok(None);
    };
    let mut reason;
    let model_derived_lead: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM scout_investigations
                 WHERE scholar_job_id = ?1 AND trigger_kind = 'model_probe')
             OR EXISTS(SELECT 1 FROM scholar_research_feedback WHERE scholar_job_id = ?1)",
            [job_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    let today: String = tx
        .query_row("SELECT date('now')", [], |row| row.get(0))
        .map_err(database_error)?;
    let question: String = tx
        .query_row(
            "SELECT question FROM scholar_jobs WHERE id = ?1",
            [job_id],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !matches!(
        finding.source_quality.as_str(),
        "primary" | "reference" | "unknown"
    ) {
        return Err(invalid("invalid Scholar source quality"));
    }
    if profile
        .verified_underlying_source_id
        .as_deref()
        .is_some_and(|id| {
            id.trim().is_empty() || id.chars().count() > 200 || id.chars().any(char::is_control)
        })
    {
        return Err(invalid("invalid verified source identity"));
    }
    let publication_is_recent = if let Some(published_at) = &profile.published_at {
        tx.query_row(
            "SELECT COALESCE(date(?1) > date('now', '-12 months')
                        AND date(?1) <= date('now'), 0)",
            [published_at],
            |row| row.get::<_, bool>(0),
        )
        .map_err(database_error)?
    } else {
        false
    };
    let source_id = profile
        .verified_underlying_source_id
        .as_deref()
        .filter(|_| !source_url.starts_with("https://en.wikipedia.org/wiki/"))
        .map(canonical);
    let corroboration = super::evidence_policy::check_corroboration(
        tx,
        finding,
        source_excerpt,
        source_url,
        job_id,
        source_id.as_deref(),
    )?;
    let time_sensitive = is_time_sensitive(&question, &finding.relation, &finding.object);
    let freshness = scholar_source_freshness(tx, finding, time_sensitive)?;
    let supported = grounded_scholar_finding(source_title, source_excerpt, finding);
    let mut semantic_id = None;
    let mut conflicting_claim_id = None;
    let mut finding_status = "needs_review";
    let assessment = super::evidence_policy::assess_scholar_finding_with_relevance(
        &question,
        source_url,
        finding,
        supported,
        freshness,
        profile,
        publication_is_recent,
        !corroboration.independent_sources.is_empty()
            && corroboration.possible_copy_of.is_none()
            && corroboration.provenance_link.is_none(),
        research_relevance,
    );
    let mut gate_status = assessment.gate_status;
    reason = Some(assessment.reason.clone());
    if let Some((earlier_id, link_reason)) = corroboration.provenance_link.as_ref() {
        if assessment.source_kind == "secondary"
            && (gate_status == "eligible" || gate_status == "awaiting_corroboration")
        {
            gate_status = "possible_shared_source";
        }
        reason = Some(format!(
            "{}; {link_reason} with finding #{earlier_id}; review before counting as independent",
            assessment.reason
        ));
    } else if let Some(earlier_id) = corroboration.possible_copy_of {
        // A warning does not claim the underlying fact is false. It
        // only blocks counting this page as independent corroboration.
        if assessment.source_kind == "secondary"
            && (gate_status == "eligible" || gate_status == "awaiting_corroboration")
        {
            gate_status = "possible_shared_source";
        }
        reason = Some(format!(
            "{}; passage substantially overlaps finding #{earlier_id}; independent reporting needs review",
            assessment.reason
        ));
    } else if let Some(earlier_id) = corroboration.shared_source_with
        && gate_status == "awaiting_corroboration"
    {
        reason = Some(format!(
            "shares verified underlying source with finding #{earlier_id}; counts as one source"
        ));
    }
    let personal_web_claim = matches!(canonical(&finding.subject).as_str(), "user" | "the user");
    if personal_web_claim {
        reason = Some("web research cannot establish a personal fact about the user".into());
        gate_status = "pending_review";
    }
    if supported
        && finding.confidence >= PROMOTION_CONFIDENCE
        && assessment.source_kind != "unknown"
        && !personal_web_claim
    {
        (semantic_id, conflicting_claim_id) = existing_semantic_match(tx, finding, &today)?;
        if let Some(id) = conflicting_claim_id {
            reason = Some(format!("conflicts with overlapping semantic claim #{id}"));
            gate_status = "conflict";
        }
    }
    if model_derived_lead && gate_status == "eligible" {
        gate_status = "pending_review";
        reason = Some("model-derived lead requires independent corroboration or review".into());
    }
    if gate_status == "eligible" && conflicting_claim_id.is_none() {
        if semantic_id.is_none() {
            tx.execute(
                        "INSERT INTO semantic_claims
                            (subject, relation, object, source, source_observation_id, created_at, valid_from, valid_until)
                         VALUES (?1, ?2, ?3, ?4, NULL, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), ?5, ?6)",
                        params![finding.subject.trim(), finding.relation.trim(), finding.object.trim(), source_url,
                            finding.valid_from, finding.valid_until],
                    ).map_err(database_error)?;
            semantic_id = Some(tx.last_insert_rowid());
        }
        finding_status = "promoted";
    }
    tx.execute(
                "INSERT INTO scholar_findings (job_id, source_title, source_url, source_excerpt,
                  subject, relation, object, evidence_text, confidence, status, semantic_claim_id, decision_reason,
                  valid_from, valid_until, source_quality, freshness, source_updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
                params![job_id, source_title, source_url, source_excerpt, finding.subject, finding.relation,
                    finding.object, finding.evidence_text, finding.confidence, finding_status, semantic_id, reason,
                    finding.valid_from, finding.valid_until, finding.source_quality, freshness, finding.source_updated_at],
            ).map_err(database_error)?;
    let finding_id = tx.last_insert_rowid();
    if let Some(support) = finding.semantic_support.as_ref() {
        if !support.supports(finding, source_excerpt) {
            return Err(invalid(
                "semantic support assessment does not match the stored finding",
            ));
        }
        tx.execute("INSERT INTO scholar_semantic_support (finding_id, assessment_json, assessed_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))", params![finding_id, serde_json::to_string(support).map_err(invalid_json)?]).map_err(database_error)?;
    }
    crate::diagnostics::event(
        "scholar.finding_disposition",
        serde_json::json!({
            "finding_id": finding_id,
            "job_id": job_id,
            "status": finding_status,
            "gate_status": gate_status,
            "confidence": finding.confidence,
            "support_method": if finding.semantic_support.is_some() { "ai_semantic_entailment" } else { "lexical" },
            "semantic_support": finding.semantic_support,
            "reason": reason,
            "source_url": source_url,
        }),
    );
    super::evidence_policy::record_assessment(
        tx,
        finding_id,
        &assessment,
        gate_status,
        reason.as_deref().unwrap_or("assessment unavailable"),
    )?;
    record_provenance_review(tx, finding_id, &corroboration)?;
    let evidence_refs = serde_json::to_string(&[source_url]).map_err(invalid_json)?;
    let payload = serde_json::to_string(&serde_json::json!({
        "subject": finding.subject,
        "relation": finding.relation,
        "object": finding.object,
        "valid_from": finding.valid_from,
        "valid_until": finding.valid_until,
        "confidence": finding.confidence,
        "source_quality": finding.source_quality,
        "freshness": freshness,
        "source_updated_at": finding.source_updated_at,
        "evidence_text": finding.evidence_text,
    }))
    .map_err(invalid_json)?;
    let proposal_id = record_role_proposal(
        tx,
        RoleProposal {
            role: "scholar",
            source_type: "scholar_finding",
            source_id: &finding_id.to_string(),
            suggested_action: "add_claim",
            target_claim_id: semantic_id.or(conflicting_claim_id),
            evidence_refs: &evidence_refs,
            payload: &payload,
        },
    )?;
    let outcome = match finding_status {
        "promoted" => "applied",
        "needs_review" if conflicting_claim_id.is_some() => "challenge_applied",
        _ => "deferred",
    };
    record_policy_decision(
        tx,
        proposal_id,
        outcome,
        reason
            .as_deref()
            .unwrap_or("accepted by source-support policy"),
        semantic_id.or(conflicting_claim_id),
        Some(if conflicting_claim_id.is_some() {
            "challenged"
        } else {
            finding_status
        }),
    )?;
    if let Some(claim_id) = semantic_id {
        link_claim_evidence(
            tx,
            claim_id,
            "web",
            Some(source_url),
            None,
            source_excerpt,
            "supports",
        )?;
        if finding_status == "promoted" && assessment.source_kind == "secondary" {
            for (earlier_id, earlier_job_id, corroborating_url, corroborating_excerpt) in
                &corroboration.independent_sources
            {
                link_claim_evidence(
                    tx,
                    claim_id,
                    "web",
                    Some(corroborating_url),
                    None,
                    corroborating_excerpt,
                    "supports",
                )?;
                let changed = tx.execute(
                            "UPDATE scholar_findings SET status = 'promoted', semantic_claim_id = ?2,
                                decision_reason = 'promoted after verified independent corroboration'
                             WHERE id = ?1 AND status = 'needs_review'",
                            params![earlier_id, claim_id],
                        ).map_err(database_error)?;
                if changed == 1 {
                    tx.execute(
                        "UPDATE scholar_jobs SET status = 'promoted', last_error = NULL
                                 WHERE id = ?1 AND status = 'needs_review'",
                        [earlier_job_id],
                    )
                    .map_err(database_error)?;
                    let earlier_proposal: Option<i64> = tx
                        .query_row(
                            "SELECT id FROM role_proposals
                                 WHERE source_type = 'scholar_finding' AND source_id = ?1
                                 ORDER BY id DESC LIMIT 1",
                            [earlier_id.to_string()],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(database_error)?;
                    if let Some(proposal_id) = earlier_proposal {
                        record_policy_decision(
                            tx,
                            proposal_id,
                            "applied",
                            "later verified independent source corroborated this finding",
                            Some(claim_id),
                            Some("promoted"),
                        )?;
                    }
                }
            }
        }
        if finding_status == "promoted" {
            tx.execute(
                        "UPDATE semantic_claims SET status = 'active',
                             checked_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             last_reviewed_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
                             next_review_at = CASE volatility
                                 WHEN 'fast_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+30 days')
                                 WHEN 'slow_changing' THEN strftime('%Y-%m-%dT%H:%M:%fZ', 'now', '+180 days')
                                 ELSE NULL END
                         WHERE id = ?1 AND retracted_by IS NULL",
                        [claim_id],
                    ).map_err(database_error)?;
        }
    } else if let Some(claim_id) = conflicting_claim_id {
        link_claim_evidence(
            tx,
            claim_id,
            "web",
            Some(source_url),
            None,
            source_excerpt,
            "contradicts",
        )?;
        tx.execute(
            "UPDATE semantic_claims SET status = 'challenged' WHERE id = ?1
                     AND status IN ('active', 'uncertain') AND retracted_by IS NULL",
            [claim_id],
        )
        .map_err(database_error)?;
    }
    Ok(Some(IngestedFinding {
        id: finding_id,
        status: finding_status,
        reason,
    }))
}

impl LearningStore {
    #[cfg(test)]
    pub fn finish_scholar_job(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
    ) -> io::Result<()> {
        self.finish_scholar_job_with_profile(
            job_id,
            source_title,
            source_url,
            source_excerpt,
            finding,
            note,
            &ScholarSourceProfile::default(),
        )
    }

    #[cfg(test)]
    pub fn finish_scholar_job_with_profile(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
        profile: &ScholarSourceProfile,
    ) -> io::Result<()> {
        self.finish_scholar_job_with_profile_and_relevance(
            job_id,
            source_title,
            source_url,
            source_excerpt,
            finding,
            note,
            profile,
            "unknown",
        )
    }

    #[cfg(test)]
    pub fn finish_scholar_job_with_profile_and_relevance(
        &mut self,
        job_id: &str,
        source_title: &str,
        source_url: &str,
        source_excerpt: &str,
        finding: Option<ScholarFinding>,
        note: Option<&str>,
        profile: &ScholarSourceProfile,
        research_relevance: &str,
    ) -> io::Result<()> {
        self.finish_scholar_job_with_evidence(
            job_id,
            &[ScholarEvidence {
                source_title: source_title.to_owned(),
                source_url: source_url.to_owned(),
                source_excerpt: source_excerpt.to_owned(),
                finding,
                profile: profile.clone(),
                research_relevance: research_relevance.to_owned(),
            }],
            note,
        )
    }

    pub fn finish_scholar_job_with_evidence(
        &mut self,
        job_id: &str,
        sources: &[ScholarEvidence],
        note: Option<&str>,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let mut job_status = "no_result";
        let mut job_reason = note.map(str::to_owned);
        let mut new_finding_ids = Vec::new();
        for source in sources {
            let Some(result) = ingest_source(&tx, job_id, source)? else {
                continue;
            };
            new_finding_ids.push(result.id);
            if job_status != "promoted" || result.status == "promoted" {
                job_status = result.status;
                job_reason = result.reason;
            }
        }
        for finding_id in new_finding_ids {
            automatically_review_source_pair(&tx, finding_id)?;
            super::corroboration::enqueue_corroboration_candidate(&tx, finding_id)?;
        }
        let has_findings: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_findings WHERE job_id = ?1)",
                [job_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        let has_unresolved_findings: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM scholar_findings
                 WHERE job_id = ?1 AND status = 'needs_review')",
                [job_id],
                |row| row.get(0),
            )
            .map_err(database_error)?;
        if has_findings && !has_unresolved_findings {
            job_status = "promoted";
            job_reason =
                Some("all directly supported findings passed automatic evidence review".into());
        }
        let changed = tx
            .execute(
                "UPDATE scholar_jobs SET status = ?2, last_error = ?3, claimed_at = NULL
             WHERE id = ?1 AND status = 'processing'",
                params![job_id, job_status, job_reason],
            )
            .map_err(database_error)?;
        if changed != 1 {
            return Err(invalid("scholar job is not processing"));
        }
        tx.commit().map_err(database_error)
    }
}

impl LearningStore {
    /// Upgrade a pending lexical assessment using an independently judged source span.
    pub(crate) fn record_scholar_semantic_support(
        &mut self,
        finding_id: i64,
        support: &crate::semantic::SemanticSupport,
    ) -> io::Result<()> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        let (mut finding, passage, title): (ScholarFinding, String, String) = tx.query_row(
            "SELECT subject, relation, object, evidence_text, confidence, valid_from, valid_until, source_quality, source_updated_at, source_excerpt, source_title FROM scholar_findings WHERE id = ?1 AND status = 'needs_review'", [finding_id],
            |row| Ok((ScholarFinding {semantic_support:None,subject:row.get(0)?,relation:row.get(1)?,object:row.get(2)?,evidence_text:row.get(3)?,confidence:row.get(4)?,valid_from:row.get(5)?,valid_until:row.get(6)?,source_quality:row.get(7)?,source_updated_at:row.get(8)?},row.get(9)?,row.get(10)?)),
        ).map_err(database_error)?;
        if !support.matches_claim(&finding) {
            return Err(invalid("semantic assessment belongs to a different claim"));
        }
        let supported = if support.verdict() == "entailed" {
            finding.evidence_text = support.evidence().to_owned();
            finding.semantic_support = Some(support.clone());
            if !grounded_scholar_finding(&title, &passage, &finding) {
                return Err(invalid(
                    "semantic assessment lacks valid original source support",
                ));
            }
            true
        } else {
            false
        };
        tx.execute("INSERT INTO scholar_semantic_support (finding_id, assessment_json, assessed_at) VALUES (?1, ?2, strftime('%Y-%m-%dT%H:%M:%fZ', 'now')) ON CONFLICT(finding_id) DO UPDATE SET assessment_json = excluded.assessment_json, assessed_at = excluded.assessed_at", params![finding_id,serde_json::to_string(support).map_err(invalid_json)?]).map_err(database_error)?;
        if supported {
            tx.execute(
                "UPDATE scholar_findings SET evidence_text = ?2 WHERE id = ?1",
                params![finding_id, finding.evidence_text],
            )
            .map_err(database_error)?;
        }
        tx.execute("UPDATE scholar_source_assessments SET direct_support = ?2,
            gate_status = CASE WHEN ?2 = 0 THEN 'unsupported' WHEN gate_status = 'unsupported' THEN 'pending_review' ELSE gate_status END,
            reason = CASE WHEN gate_status = 'conflict' THEN reason || '. ' || ?3 ELSE ?3 END WHERE finding_id = ?1", params![finding_id,supported,format!("AI semantic {}: {}",support.verdict(),support.reason())]).map_err(database_error)?;
        tx.commit().map_err(database_error)?;
        crate::diagnostics::event(
            "scholar.semantic_support_recorded",
            serde_json::json!({"finding_id":finding_id,"direct_support":supported,"assessment":support}),
        );
        Ok(())
    }
}
